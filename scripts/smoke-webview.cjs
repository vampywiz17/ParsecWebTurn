// Exercise the actual packaged WebView2 application in a disposable profile.
// CDP exists only in this test process, never in normal application launches.
const fs = require('node:fs');
const path = require('node:path');
const http = require('node:http');
const assert = require('node:assert/strict');
const { spawn, execFileSync } = require('node:child_process');
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
const repository = path.resolve(__dirname, '..');
// Synthetic tests must not contact GitHub or download an actual release.
process.env.PARSECWEBTURN_NO_UPDATE_CHECK = '1';

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
  const mediaEvents = [];
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
      if(message.method==='Media.playerPropertiesChanged') mediaEvents.push(message.params.properties);
      if(message.method==='Media.playerMessagesLogged') mediaEvents.push(message.params.messages.filter(item=>/decoder|Initialized/i.test(item.message) && !/LUID|adapter/i.test(item.message)));
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
    const input={provider:'custom',customUrls:['stun:127.0.0.1:9','turns:relay.example.invalid'],customUsername:'smoke-user',customPassword:'smoke-secret',
      turnKeyId:'',apiToken:'',cacheCredentials:false,ttl:86400};
    await invoke(main,'save_configuration',{input});
    const saved=await invoke(main,'get_configuration');
    assert.equal(saved.provider,'custom'); assert.equal(saved.hasCustomPassword,true);
    assert.ok(!JSON.stringify(saved).includes('smoke-secret'));
    const disk=fs.readFileSync(path.join(root,'settings.json'),'utf8');
    assert.ok(!disk.includes('smoke-secret')); assert.ok(JSON.parse(disk).encryptedCustomPassword);
    await evaluate(main,`document.getElementById('provider').value='custom';document.getElementById('urls').value=${JSON.stringify(input.customUrls.join('\n'))};document.getElementById('username').value='smoke-user-ui';document.getElementById('provider').dispatchEvent(new Event('change'));document.getElementById('save').click()`);
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
    assert.equal(await evaluate(parsec,`document.documentElement.requestFullscreen().then(()=>false,error=>error.name==='NotAllowedError')`),true,'Automatic web fullscreen must be blocked');
    const fullscreenMode = await invoke(main,'set_parsec_window_mode',{fullscreen:true});
    assert.equal(fullscreenMode.fullscreen,true); assert.equal(fullscreenMode.menuVisible,false);
    await send('Input.dispatchKeyEvent',{type:'keyDown',key:'W',code:'KeyW',windowsVirtualKeyCode:87,modifiers:10},parsec);
    await send('Input.dispatchKeyEvent',{type:'keyUp',key:'W',code:'KeyW',windowsVirtualKeyCode:87,modifiers:10},parsec);
    await delay(300);
    const recovered = await evaluate(parsec,`window.__TAURI_INTERNALS__.invoke('parsec_window_shortcut',{toggle:true})`);
    assert.equal(recovered.fullscreen,true,'Ctrl+Shift+W must restore the focused WebView before toggling fullscreen again');
    assert.equal(recovered.menuVisible,false);
    // If Ctrl+Shift+W restored windowed mode, this toggle enters fullscreen again.
    // Verify the native state from the local interface before returning windowed.
    const windowedMode = await invoke(main,'set_parsec_window_mode',{fullscreen:false});
    assert.equal(windowedMode.fullscreen,false); assert.equal(windowedMode.menuVisible,true); assert.equal(windowedMode.decorated,true);
    const rejected=await evaluate(parsec,`window.__TAURI_INTERNALS__.invoke('get_configuration').then(()=>false,()=>true)`);
    assert.equal(rejected,true,'Remote Parsec page must not access settings');
    const rejectedWrite=await evaluate(parsec,`window.__TAURI_INTERNALS__.invoke('save_configuration',{input:${JSON.stringify(input)}}).then(()=>false,()=>true)`);
    assert.equal(rejectedWrite,true,'Remote Parsec page must not modify settings');
    const rejectedShow=await evaluate(parsec,`window.__TAURI_INTERNALS__.invoke('show_configuration').then(()=>false,()=>true)`);
    assert.equal(rejectedShow,true,'Remote Parsec page must not reveal settings');
    for (const command of ['get_update','check_update','install_update','dismiss_update']) {
      const denied=await evaluate(parsec,`window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)}).then(()=>false,()=>true)`);
      assert.equal(denied,true,`Remote page must not invoke ${command}`);
    }
    await evaluate(main,`document.getElementById('stats').click()`);
    const stats=await find('/stats.html');
    let sample;
    for(let i=0;i<100;i++){sample=await invoke(stats,'get_stats');if(sample.inboundMbps>0 && sample.outboundMbps>0)break;await delay(100);}
    assert.equal(sample.state,'connected');assert.ok(sample.inboundMbps>0);assert.ok(sample.outboundMbps>0);
    assert.equal(sample.route,'direct');assert.ok(Number.isFinite(sample.rttMs));
    assert.equal(sample.configuredTurnUsed,false,'An available TURN configuration does not imply actual use');
    assert.equal(sample.codec,null,'Data-channel-only video metadata must stay unknown');
    assert.equal(sample.fps,null);
    assert.equal(sample.stale,false);
    const compatibility = await evaluate(parsec,`(() => {
      const Native = Object.getPrototypeOf(window.RTCPeerConnection);
      const inspect = (Constructor, config, setter=false) => {
        let peer;
        try {
          peer=new Constructor(setter ? {} : config);
          if(setter)peer.setConfiguration(config);
          const value=peer.getConfiguration();
          return {policy:value.iceTransportPolicy,bundle:value.bundlePolicy,pool:value.iceCandidatePoolSize};
        } catch(error) { return {error:error.name}; }
        finally {peer?.close();}
      };
      const inputs={
        inherited:Object.create({iceTransportPolicy:'relay',bundlePolicy:'max-bundle'}),
        nonEnumerable:Object.defineProperty({},'iceTransportPolicy',{value:'relay'}),
        primitive:42,
        unrelatedGetter:Object.defineProperty({},'unrelated',{enumerable:true,get(){throw Error('Unexpected getter');}}),
        frozen:Object.freeze({iceTransportPolicy:'relay',iceServers:[]}),
      };
      return Object.entries(inputs).map(([name,config])=>({name,
        native:inspect(Native,config),injected:inspect(window.RTCPeerConnection,config),
        nativeSetter:inspect(Native,config,true),injectedSetter:inspect(window.RTCPeerConnection,config,true)}));
    })()`);
    for(const item of compatibility) {
      assert.deepEqual(item.injected,item.native,'Native constructor compatibility: '+item.name);
      assert.deepEqual(item.injectedSetter,item.nativeSetter,'Native setConfiguration compatibility: '+item.name);
    }
    // Inject synthetic Chromium prflx/TURN metadata into the real loopback stats.
    // This verifies the native DTO and panel, not a real external TURN session.
    await evaluate(parsec,`for(const peer of [first,second]) {
      peer.smokeOriginalGetStats=peer.getStats.bind(peer);
      peer.getStats=async(...args)=>{
        const nativeReport=await peer.smokeOriginalGetStats(...args);
        // RTCStatsReport.get() can return fresh dictionaries. Keep synthetic
        // changes in our own Map rather than mutating a temporary dictionary.
        const report=new Map([...nativeReport].map(([id,value])=>[id,{...value}]));
        const transport=[...report.values()].find(value=>value.type==='transport' && value.selectedCandidatePairId);
        const pair=transport && report.get(transport.selectedCandidatePairId);
        if(pair)Object.assign(report.get(pair.localCandidateId),{candidateType:'prflx',relayProtocol:'tls',url:'turns:relay.example.invalid:5349?transport=tcp'});
        return report;
      };
    }`);
    for(let i=0;i<100;i++){sample=await invoke(stats,'get_stats');if(sample.route==='relay' && sample.configuredTurnUsed===true)break;await delay(100);}
    assert.equal(sample.route,'relay');assert.equal(sample.configuredTurnUsed,true);
    assert.equal(sample.turnServer,'turns:relay.example.invalid');assert.equal(sample.turnProtocol,'tls');
    assert.equal(sample.protocol,'udp');assert.equal(sample.localCandidateType,'prflx');
    await delay(1100);
    assert.ok(await evaluate(stats,`document.getElementById('details').textContent.includes('Configured TURN in use: Yes')`));
    await evaluate(parsec,`for(const peer of [first,second])peer.getStats=peer.smokeOriginalGetStats`);
    for(let i=0;i<100;i++){sample=await invoke(stats,'get_stats');if(sample.route==='direct')break;await delay(100);}
    assert.equal(sample.route,'direct');assert.equal(sample.configuredTurnUsed,false,'Route changes must clear old TURN evidence');
    await send('Media.enable',{},parsec);
    await evaluate(parsec,`(async()=>{
      const decoder = new VideoDecoder({output(frame){frame.close();},error(error){window.smoke.videoError=String(error);}});
      const encoder = new VideoEncoder({output(chunk,metadata){
        if(decoder.state==='unconfigured')decoder.configure(metadata.decoderConfig || {codec:'vp8',codedWidth:64,codedHeight:64});
        decoder.decode(chunk);
      },error(error){window.smoke.videoError=String(error);}});
      encoder.configure({codec:'vp8',width:64,height:64,latencyMode:'realtime',hardwareAcceleration:'prefer-software'});
      const canvas = new OffscreenCanvas(64,64);canvas.getContext('2d').fillRect(0,0,64,64);
      const frame = new VideoFrame(canvas,{timestamp:0});encoder.encode(frame,{keyFrame:true});frame.close();
      await encoder.flush();await decoder.flush();window.smoke.decoder=decoder;window.smoke.encoder=encoder;
      let timestamp=0;
      window.smoke.videoTimer=setInterval(()=>{
        if(encoder.encodeQueueSize>2)return;
        const frame=new VideoFrame(canvas,{timestamp:timestamp+=50000});
        encoder.encode(frame,{keyFrame:timestamp===50000});frame.close();
      },50);
    })()`);
    for(let i=0;i<100;i++){sample=await invoke(stats,'get_stats');if(sample.videoSource==='Chromium Media' && sample.codec==='VP8' && sample.decoder && sample.width===64 && sample.height===64)break;await delay(100);}
    if(!sample.decoder) console.log('Synthetic video Media events: '+JSON.stringify(mediaEvents));
    assert.equal(sample.videoSource,'Chromium Media','Native Media events must supplement data-channel WebRTC statistics');
    assert.equal(sample.codec,'VP8'); assert.ok(sample.decoder); assert.equal(sample.width,64); assert.equal(sample.height,64);
    for(let i=0;i<100;i++){sample=await invoke(stats,'get_stats');if(sample.fps>5 && sample.fpsSource==='WebCodecs decoder')break;await delay(100);}
    assert.ok(sample.fps>5 && sample.fps<100,'Real decoded frames must produce a plausible FPS rate: '+sample.fps);
    assert.equal(sample.fpsSource,'WebCodecs decoder');assert.equal(sample.packetsLost,null,'Data-channel traffic does not expose RTP packet loss');
    if(process.env.PARSECWEBTURN_SCREENSHOTS) {await delay(1200);const {data}=await send('Page.captureScreenshot',{},stats);fs.writeFileSync(path.join(repository,'tauri-stats.png'),Buffer.from(data,'base64'));}
    assert.equal(errors.length,0,String(errors));
    // Reconnect replaces a native window internally; this must not exit the app.
    await invoke(main,'connect_saved');
    await find('https://web.parsec.app');
    assert.equal(child.exitCode,null,'Internal Parsec window replacement must preserve the app');
    // Exercise the Windows X-button path with settings hidden and stats open.
    // Closing the browser engine would not exercise the native app-close event.
    execFileSync('powershell.exe',['-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File',
      path.join(repository,'scripts/test-window-close.ps1'),'-TestProcessId',String(child.pid)],{windowsHide:true});
    for(let i=0;i<100 && child.exitCode===null;i++)await delay(100);
    assert.equal(child.exitCode,0,'Closing Parsec must exit the app instead of reopening settings');
    execFileSync('powershell.exe',['-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File',
      path.join(repository,'scripts/test-startup.ps1'),'-ExecutablePath',path.resolve(process.argv[2] || path.join(repository,'ParsecWebTurn.exe')),
      '-SavedProfile',root],{windowsHide:true,stdio:['ignore','pipe','pipe']});
    execFileSync('powershell.exe',['-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File',
      path.join(repository,'scripts/test-update.ps1'),'-ExecutablePath',path.resolve(process.argv[2] || path.join(repository,'ParsecWebTurn.exe')),
      '-SavedProfile',root],{windowsHide:true,stdio:['ignore','pipe','pipe']});
    console.log('PASS: native WebView2 settings, DPAPI save, WebIDL compatibility, WebRTC traffic/RTT, decoded FPS, window recovery, remote IPC isolation, reconnect, X-button app exit, startup visibility and portable update replacement/restart');
  } finally {
    socket?.close();
    if(child.exitCode===null) child.kill();
    if(process.env.GITHUB_ACTIONS==='true') {
      execFileSync('reg.exe',['delete',ciPolicy,'/v','ParsecWebTurn.exe','/f']);
    }
    // Keep the disposable profile for inspecting a failed smoke test.
  }
})().catch(error=>{console.error(error);process.exitCode=1;});
