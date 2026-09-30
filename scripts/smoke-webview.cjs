// Exercise the actual packaged WebView2 application in a disposable profile.
// CDP exists only in this test process, never in normal application launches.
const fs = require('node:fs');
const path = require('node:path');
const http = require('node:http');
const assert = require('node:assert/strict');
const { spawn, execFileSync } = require('node:child_process');
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
const repository = path.resolve(__dirname, '..');

const page = `<!doctype html><html><body><h1>Isolated WebRTC smoke test</h1><script>
window.smoke = { installedBeforePageScript: !!window.__parsecWebTurnPatched };
const first = new RTCPeerConnection({iceServers:[{urls:'stun:original.invalid'}],iceTransportPolicy:'all'});
window.smoke.servers = first.getConfiguration().iceServers;
window.smoke.policy = first.getConfiguration().iceTransportPolicy;
first.setConfiguration({iceServers:[{urls:'stun:replacement.invalid'}],iceTransportPolicy:'all'});
window.smoke.after = first.getConfiguration().iceServers;
const second = new RTCPeerConnection();
// The configured dummy STUN endpoint tests the override without external traffic.
// Remove it via the native prototype for the loopback fixture only, so gathering
// does not wait for STUN retries. Both patched peers remain tracked for telemetry.
const nativeSetConfiguration = Object.getPrototypeOf(RTCPeerConnection.prototype).setConfiguration;
nativeSetConfiguration.call(first,{iceServers:[]});
nativeSetConfiguration.call(second,{iceServers:[]});
const gather = async peer => { for(let i=0;i<200;i++){ if(peer.iceGatheringState==='complete')return; await new Promise(resolve=>setTimeout(resolve,50)); } throw new Error('ICE gathering timeout'); };
second.ondatachannel = ({channel}) => { window.smoke.receiver = channel; channel.onmessage = ()=>{}; };
const channel = first.createDataChannel('smoke');
window.smoke.sender = channel;
channel.onopen = () => { window.smoke.connected = true; setInterval(()=>{ if(channel.readyState==='open' && channel.bufferedAmount<1000000) channel.send(new Uint8Array(16000)); },100); };
(async()=>{ await first.setLocalDescription(await first.createOffer()); await gather(first); await second.setRemoteDescription(first.localDescription);
await second.setLocalDescription(await second.createAnswer()); await gather(second); await first.setRemoteDescription(second.localDescription);
})().catch(error=>window.smoke.error=String(error));
</script></body></html>`;

(async () => {
  const root = fs.mkdtempSync(path.join(repository, 'dist', 'smoke-'));
  const portServer = http.createServer();
  await new Promise(resolve => portServer.listen(0, '127.0.0.1', resolve));
  const port = portServer.address().port;
  await new Promise(resolve => portServer.close(resolve));
  const browserArguments = `--remote-debugging-port=${port} --disable-features=msWebOOUI,msPdfOOUI,WebRtcHideLocalIpsWithMdns`;
  const ciPolicy = 'HKLM\\Software\\Policies\\Microsoft\\Edge\\WebView2\\AdditionalBrowserArguments';
  // WebView2 150+ ignores WEBVIEW2_* environment overrides for elevated hosts.
  // GitHub runners are elevated; apply Microsoft's documented HKLM alternative
  // to this executable only, on the disposable runner, then remove it below.
  // https://github.com/MicrosoftEdge/WebView2Feedback/issues/5640
  if(process.env.GITHUB_ACTIONS==='true') {
    execFileSync('reg.exe',['add',ciPolicy,'/v','ParsecWebTurn.exe','/t','REG_SZ','/d',browserArguments,'/f']);
  }
  const child = spawn(path.resolve(process.argv[2] || path.join(repository, 'ParsecWebTurn.exe')),
    ['--settings', '--data-dir', root], {
      // mDNS can be blocked by a CI/local firewall even for loopback peers.
      // Disable candidate obfuscation only in this isolated mock-page test.
      env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: browserArguments },
      windowsHide: true, stdio: ['ignore', 'ignore', 'pipe'],
    });
  let startupError = '';
  child.stderr.on('data',data=>{startupError=(startupError+data.toString()).slice(-4096);});
  let socket;
  const pending = new Map();
  let sequence = 0;
  const errors = [];
  const sessions = new Map();
  try {
    let browser;
    for (let i=0; i<150; i++) {
      try { browser = await (await fetch(`http://127.0.0.1:${port}/json/version`)).json(); if(browser.webSocketDebuggerUrl) break; } catch {}
      if(child.exitCode !== null) throw new Error('Application exited before creating WebView2');
      await delay(100);
    }
    if(!browser?.webSocketDebuggerUrl && process.env.GITHUB_ACTIONS==='true') {
      // Runner diagnostics contain only process/runtime configuration, never
      // real accounts: this entire test uses a disposable synthetic profile.
      console.log(execFileSync('powershell.exe',['-NoProfile','-Command',
        "Get-CimInstance Win32_Process | Where-Object { $_.Name -in @('ParsecWebTurn.exe','msedgewebview2.exe') } | Select-Object Name,ProcessId,ParentProcessId,CommandLine | ConvertTo-Json -Depth 3; Get-ChildItem Env:WEBVIEW2* | Format-List; Get-ItemProperty 'HKLM:/SOFTWARE/Policies/Microsoft/Edge','HKCU:/SOFTWARE/Policies/Microsoft/Edge' -ErrorAction SilentlyContinue | Select-Object DeveloperToolsAvailability,RemoteDebuggingAllowed | ConvertTo-Json"
      ],{encoding:'utf8'}));
    }
    assert.ok(browser?.webSocketDebuggerUrl, 'WebView2 debugging endpoint was not created. '+startupError);
    socket = new WebSocket(browser.webSocketDebuggerUrl);
    await new Promise((resolve,reject) => { socket.addEventListener('open',resolve,{once:true}); socket.addEventListener('error',reject,{once:true}); });
    const send = (method, params={}, sessionId) => new Promise((resolve,reject) => {
      const id = ++sequence;
      const timer = setTimeout(()=>{ pending.delete(id); reject(new Error(`CDP timeout: ${method}`)); },20000);
      pending.set(id,{resolve:value=>{clearTimeout(timer);resolve(value);},reject:error=>{clearTimeout(timer);reject(error);}});
      socket.send(JSON.stringify({id,method,params,...(sessionId?{sessionId}:{})}));
    });
    socket.addEventListener('message', ({data}) => {
      const message = JSON.parse(data);
      if(message.id) { const task=pending.get(message.id); if(task){ pending.delete(message.id); message.error?task.reject(new Error(JSON.stringify(message.error))):task.resolve(message.result); } return; }
      if(message.method==='Target.attachedToTarget') {
        const {sessionId,targetInfo} = message.params;
        sessions.set(targetInfo.targetId,sessionId);
        (async()=>{
          if(targetInfo.type==='page') await send('Fetch.enable',{patterns:[{urlPattern:'https://web.parsec.app/*',resourceType:'Document',requestStage:'Request'}]},sessionId);
          await send('Runtime.runIfWaitingForDebugger',{},sessionId);
        })().catch(error=>errors.push(error));
      }
      if(message.method==='Fetch.requestPaused') {
        send('Fetch.fulfillRequest',{requestId:message.params.requestId,responseCode:200,
          responseHeaders:[{name:'Content-Type',value:'text/html; charset=utf-8'}],body:Buffer.from(page).toString('base64')},message.sessionId).catch(error=>errors.push(error));
      }
    });
    const evaluate = async (sessionId, expression) => {
      const result = await send('Runtime.evaluate',{expression,awaitPromise:true,returnByValue:true},sessionId);
      if(result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
      return result.result.value;
    };
    const invoke = (sessionId,command,args={}) => evaluate(sessionId,`window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)},${JSON.stringify(args)})`);
    const find = async suffix => {
      for(let i=0;i<150;i++) {
        const {targetInfos} = await send('Target.getTargets');
        const target = targetInfos.find(info=>info.type==='page' && (info.url.includes(suffix) ||
          (suffix==='/index.html' && /^(?:https?:\/\/tauri\.localhost|tauri:\/\/localhost)\/$/.test(info.url))));
        if(target) {
          let sessionId=sessions.get(target.targetId);
          if(!sessionId) { ({sessionId}=await send('Target.attachToTarget',{targetId:target.targetId,flatten:true})); sessions.set(target.targetId,sessionId); }
          // CDP exposes the target URL before initialization/deferred scripts
          // finish, especially on a slower CI runner. Wait for the real IPC
          // bridge and document before issuing native commands.
          const ready = await evaluate(sessionId,`!!window.__TAURI_INTERNALS__?.invoke && document.readyState==='complete' &&
            (!document.getElementById('save') || (!document.getElementById('save').disabled && !!document.getElementById('version').textContent))`);
          if(ready) return sessionId;
        }
        await delay(100);
      }
      throw new Error(`No webview found for ${suffix}`);
    };
    const main = await find('/index.html');
    const initial = await invoke(main,'get_configuration');
    assert.equal(initial.version,fs.readFileSync(path.join(repository,'VERSION'),'utf8').trim());
    assert.equal(initial.hasApiToken,false);
    const input={provider:'custom',customUrls:['stun:127.0.0.1:9'],customUsername:'smoke-user',customPassword:'smoke-secret',
      turnKeyId:'',apiToken:'',cacheCredentials:false,ttl:86400};
    await invoke(main,'save_configuration',{input});
    const saved=await invoke(main,'get_configuration');
    assert.equal(saved.provider,'custom'); assert.equal(saved.hasCustomPassword,true);
    assert.ok(!JSON.stringify(saved).includes('smoke-secret'));
    const disk=fs.readFileSync(path.join(root,'settings.json'),'utf8');
    assert.ok(!disk.includes('smoke-secret')); assert.ok(JSON.parse(disk).encryptedCustomPassword);
    await evaluate(main,`document.getElementById('provider').value='custom';document.getElementById('urls').value='stun:127.0.0.1:9';document.getElementById('username').value='smoke-user-ui';document.getElementById('provider').dispatchEvent(new Event('change'));document.getElementById('save').click()`);
    let uiSaved = false;
    for(let i=0;i<100;i++) {uiSaved=await evaluate(main,`document.getElementById('status').textContent.startsWith('Settings saved')`);if(uiSaved)break;await delay(100);}
    assert.equal(uiSaved,true,'The real settings form must save successfully');
    const uiConfig = await invoke(main,'get_configuration');
    assert.equal(uiConfig.customUsername,'smoke-user-ui');
    assert.equal(uiConfig.hasCustomPassword,true,'Blank password field must retain the saved secret');
    if(process.env.PARSECWEBTURN_SCREENSHOTS) {
      const {data}=await send('Page.captureScreenshot',{},main);
      fs.writeFileSync(path.join(repository,'tauri-settings.png'),Buffer.from(data,'base64'));
    }
    await send('Target.setAutoAttach',{autoAttach:true,waitForDebuggerOnStart:true,flatten:true});
    await invoke(main,'connect_saved');
    const parsec=await find('https://web.parsec.app');
    let smoke;
    for(let i=0;i<300;i++){smoke=await evaluate(parsec,'window.smoke && ({installedBeforePageScript:window.smoke.installedBeforePageScript,servers:window.smoke.servers,after:window.smoke.after,policy:window.smoke.policy,connected:window.smoke.connected,error:window.smoke.error})'); if(smoke?.connected)break;await delay(100);}
    assert.equal(smoke?.installedBeforePageScript,true);
    assert.equal(smoke.policy,'all');
    assert.equal(smoke.servers[0].urls[0],'stun:127.0.0.1:9');
    assert.equal(smoke.after[0].urls[0],'stun:127.0.0.1:9');
    if(!smoke.connected) {
      const diagnostics = await evaluate(parsec,`(async()=>({error:window.smoke.error,first:first.connectionState,second:second.connectionState,firstIce:first.iceConnectionState,secondIce:second.iceConnectionState,channel:channel.readyState,reports:await Promise.all([first,second].map(async peer=>[...await peer.getStats()].map(([,value])=>({type:value.type,state:value.state,candidateType:value.candidateType,protocol:value.protocol,mdns:value.address?.endsWith('.local'),requestsSent:value.requestsSent,responsesReceived:value.responsesReceived}))))}))()`);
      throw new Error('Loopback WebRTC did not connect: '+JSON.stringify(diagnostics));
    }
    const rejected=await evaluate(parsec,`window.__TAURI_INTERNALS__.invoke('get_configuration').then(()=>false,()=>true)`);
    assert.equal(rejected,true,'Remote Parsec page must not access settings');
    const rejectedWrite=await evaluate(parsec,`window.__TAURI_INTERNALS__.invoke('save_configuration',{input:${JSON.stringify(input)}}).then(()=>false,()=>true)`);
    assert.equal(rejectedWrite,true,'Remote Parsec page must not modify settings');
    await evaluate(main,`document.getElementById('stats').click()`);
    const stats=await find('/stats.html');
    let sample;
    for(let i=0;i<100;i++){sample=await invoke(stats,'get_stats');if(sample.inboundMbps>0 && sample.outboundMbps>0)break;await delay(100);}
    assert.equal(sample.state,'connected');assert.ok(sample.inboundMbps>0);assert.ok(sample.outboundMbps>0);
    assert.equal(sample.route,'direct');assert.ok(Number.isFinite(sample.rttMs));
    assert.equal(sample.codec,null,'Data-channel-only video metadata must stay unknown');
    assert.equal(sample.fps,null);
    assert.equal(sample.stale,false);
    if(process.env.PARSECWEBTURN_SCREENSHOTS) {await delay(1200);const {data}=await send('Page.captureScreenshot',{},stats);fs.writeFileSync(path.join(repository,'tauri-stats.png'),Buffer.from(data,'base64'));}
    assert.equal(errors.length,0,String(errors));
    console.log('PASS: native WebView2 settings, DPAPI save, document-start injection, direct WebRTC traffic/RTT, statistics panel and remote IPC isolation');
    socket.send(JSON.stringify({id:++sequence,method:'Browser.close'}));
    await delay(500);
  } finally {
    socket?.close();
    if(child.exitCode===null) child.kill();
    if(process.env.GITHUB_ACTIONS==='true') {
      execFileSync('reg.exe',['delete',ciPolicy,'/v','ParsecWebTurn.exe','/f']);
    }
    // Keep the disposable profile for inspecting a failed smoke test.
  }
})().catch(error=>{console.error(error);process.exitCode=1;});
