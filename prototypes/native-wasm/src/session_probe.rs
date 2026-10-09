//! Controlled WASM guest + local native peer. No account, original UI attempt,
//! Parsec host protocol or video decoder is exercised by this diagnostic.
use crate::signaling::{Candidate, CandidateGate, Credentials, Description};
use anyhow::{bail, Context, Result};
use bytes::Bytes;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use wasmtime::{Config, Engine, Module};
use webrtc::{
    api::{setting_engine::SettingEngine, APIBuilder},
    data_channel::{
        data_channel_init::RTCDataChannelInit, data_channel_state::RTCDataChannelState,
    },
    ice::network_type::NetworkType,
    peer_connection::{
        configuration::RTCConfiguration, peer_connection_state::RTCPeerConnectionState,
        sdp::session_description::RTCSessionDescription,
    },
};

pub fn probe() -> Result<serde_json::Value> {
    probe_mode(Mode::Transport, false)
}

pub fn control_probe() -> Result<serde_json::Value> {
    probe_mode(Mode::Control, false)
}

pub fn buffer_probe() -> Result<serde_json::Value> {
    probe_mode(Mode::Buffers, false)
}

pub fn dtls_failure_probe(legacy_rsa_1024: bool) -> Result<serde_json::Value> {
    if !crate::transport_diagnostics::enable() {
        bail!("controlled DTLS diagnostic logger unavailable");
    }
    let mut report = probe_mode(Mode::DtlsFailure, legacy_rsa_1024)?;
    if !crate::transport_diagnostics::contains_reason("certificate-fingerprint-mismatch")
        || !crate::transport_diagnostics::contains_reason("verify-ecdsa-p256-sha256")
    {
        bail!("actual native DTLS diagnostic categories were not observed");
    }
    report["legacy_rsa_1024_enabled"] = serde_json::json!(legacy_rsa_1024);
    report["library_failure_classification_verified"] = serde_json::json!(true);
    report["signature_algorithm_reporting_verified"] = serde_json::json!(true);
    report["native_transport_diagnostics"] =
        serde_json::to_value(crate::transport_diagnostics::snapshot(true))?;
    Ok(report)
}

enum Mode {
    Transport,
    Control,
    Buffers,
    DtlsFailure,
}

fn probe_mode(mode: Mode, legacy_rsa_1024: bool) -> Result<serde_json::Value> {
    let dtls_failure = matches!(mode, Mode::DtlsFailure);
    let control = !matches!(mode, Mode::Transport) && !dtls_failure;
    let buffers = matches!(mode, Mode::Buffers);
    let mut config = Config::new();
    config
        .wasm_threads(true)
        .consume_fuel(true)
        .epoch_interruption(true);
    let engine = Engine::new(&config)?;
    let module = Module::new(
        &engine,
        r#"(module
      (import "env" "memory" (memory 1 1 shared))
      (import "env" "parsec_web_init" (func $init))
      (import "env" "parsec_web_new_attempt" (func $offer (param i32 i32 i32 i32 i32 i32 i32)))
      (import "env" "parsec_web_begin_p2p" (func $begin (param i32 i32 i32 i32 i32)))
      (import "env" "parsec_web_add_candidate" (func $candidate (param i32 i32 i32 i32 i32)))
      (import "env" "parsec_web_poll_events" (func $poll (param i32 i32) (result i32)))
      (import "env" "parsec_web_destroy" (func $destroy))
      (import "env" "parsec_client_set_config" (func $config (param i32 i32 i32 i32)))
      (import "env" "parsec_web_get_status" (func $status (result i32)))
      (import "env" "parsec_web_send_message" (func $send (param i32)))
      (import "env" "parsec_web_send_user_data" (func $send_user (param i32 i32)))
      (import "env" "parsec_web_get_buffer_size" (func $buffer_size (param i32) (result i32)))
      (import "env" "parsec_web_get_buffer" (func $buffer (param i32 i32)))
      (import "env" "parsec_web_disconnect" (func $disconnect (param i32 i32)))
      (import "env" "parsec_web_get_guests" (func $guests (param i32 i32)))
      (import "env" "parsec_web_get_self" (func $self (param i32 i32) (result i32)))
      (import "env" "parsec_web_get_host_mode" (func $mode (result i32)))
      (import "env" "parsec_web_get_metrics" (func $metrics (param i32 i32 i32 i32 i32 i32 i32)))
      (data (i32.const 16) "local-session-test\00")
      (func (export "offer") (param $control i32) (result i32)
        call $init
        local.get $control if i32.const 1 i32.const 16 i32.const 0 i32.const 12 call $config end
        i32.const 16 i32.const 100 i32.const 400 i32.const 700 i32.const 256 i32.const 64 i32.const 80 call $offer
        i32.const 64 i32.const 0 i32.const 1 i32.atomic.rmw.cmpxchg i32.eqz
        if i32.const 64 i32.const 1 i64.const 4000000000 memory.atomic.wait32
          i32.const 2 i32.eq if unreachable end end
        i32.const 64 i32.const 0 i32.atomic.store i32.const 80 i32.load)
      (func (export "begin")
        i32.const 16 i32.const 0 i32.const 1000 i32.const 1300 i32.const 1600 call $begin)
      (func (export "candidate") (param $port i32)
        i32.const 16 i32.const 2000 local.get $port i32.const 0 i32.const 0 call $candidate)
      (func (export "sync")
        i32.const 16 i32.const -1 i32.const -1 i32.const 1 i32.const 0 call $candidate)
      (func (export "poll") (param $capacity i32) (result i32)
        i32.const 4096 local.get $capacity call $poll)
      (func (export "status") (result i32) call $status)
      (func (export "send") i32.const 8192 call $send)
      (func (export "send_user") (param $id i32) local.get $id i32.const 8192 call $send_user)
      (func (export "buffer_size") (param $key i32) (result i32) local.get $key call $buffer_size)
      (func (export "buffer") (param $key i32) (param $ptr i32) local.get $key local.get $ptr call $buffer)
      (func (export "disconnect") i32.const -3 i32.const 8 call $disconnect)
      (func (export "guests") i32.const 10000 i32.const 4096 call $guests)
      (func (export "self") (result i32) i32.const 14300 i32.const 14304 call $self)
      (func (export "mode") (result i32) call $mode)
      (func (export "metrics") i32.const 14400 i32.const 14404 i32.const 14408 i32.const 14409
         i32.const 14412 i32.const 14416 i32.const 14420 call $metrics)
      (func (export "destroy") call $destroy))"#,
    )?;
    let (mut store, instance) = crate::instantiate(&engine, &module)?;
    // This fixture has fuel-limited straight-line exports and a four-second
    // atomic wait. Its native operations have separate deadlines. Allow calls
    // after the generic bootstrap's single five-second watchdog epoch.
    store.set_epoch_deadline(2);
    let memory = store.data().memory.clone();
    let backend = store.data().backend.clone();
    backend
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .legacy_rsa_1024_enabled = legacy_rsa_1024;
    if instance
        .get_typed_func::<i32, i32>(&mut store, "offer")?
        .call(&mut store, i32::from(control))?
        != 0
    {
        bail!("guest offer failed");
    }
    let client_credentials = Credentials {
        ufrag: memory.string(100, 256)?,
        password: memory.string(400, 256)?,
        fingerprint: memory.string(700, 256)?,
    };
    let mid = backend
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .native_attempt
        .as_ref()
        .context("guest attempt absent")?
        .snapshot()["mid"]
        .as_str()
        .context("offer MID absent")?
        .to_owned();
    let compact = Description {
        credentials: client_credentials,
        mid,
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let mut settings = SettingEngine::default();
    settings.set_network_types(vec![NetworkType::Udp4]);
    if !control {
        // Synthetic peer only. Exercise the reported padded-ufrag shape with
        // real STUN authentication, DTLS and SCTP, without editing SDP tokens.
        settings.set_ice_credentials("dGVzdA==".into(), "abcdefghijklmnopqrstuv1234567890".into());
    }
    let api = APIBuilder::new().with_setting_engine(settings).build();
    let peer = Arc::new(runtime.block_on(async {
        tokio::time::timeout(
            Duration::from_secs(5),
            api.new_peer_connection(RTCConfiguration::default()),
        )
        .await
    })??);
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let mut channels = Vec::new();
    let checked = (|| -> Result<serde_json::Value> {
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), async {
                for (id, label) in crate::transport::CHANNELS {
                    let channel = peer
                        .create_data_channel(
                            label,
                            Some(RTCDataChannelInit {
                                negotiated: Some(id),
                                ordered: Some(true),
                                ..Default::default()
                            }),
                        )
                        .await?;
                    let tx = tx.clone();
                    channel.on_message(Box::new(move |message| {
                        let tx = tx.clone();
                        Box::pin(async move {
                            let _ = tx.try_send((id, message.is_string, message.data));
                        })
                    }));
                    channels.push(channel);
                }
                let sdp = compact
                    .answer(&compact.credentials)?
                    .replace("a=setup:active", "a=setup:actpass");
                peer.set_remote_description(RTCSessionDescription::offer(sdp)?)
                    .await?;
                let mut gathered = peer.gathering_complete_promise().await;
                let answer = peer.create_answer(None).await?;
                peer.set_local_description(answer).await?;
                gathered.recv().await;
                Ok::<(), anyhow::Error>(())
            })
            .await
        })??;
        let answer = runtime
            .block_on(peer.local_description())
            .context("server answer absent")?;
        let mut remote = Description::from_parsec_remote_sdp(&answer.sdp)?;
        if dtls_failure {
            // Synthetic negative peer: keep ICE valid but advertise a different
            // certificate digest. Native fingerprint verification must reject it.
            remote.credentials.fingerprint = format!("sha-256 {}", ["00"; 32].join(":"));
        }
        for (ptr, value) in [
            (1000, &remote.credentials.ufrag),
            (1300, &remote.credentials.password),
            (1600, &remote.credentials.fingerprint),
        ] {
            // Exercise the pinned JS LF-split representation through the real
            // guest import; control/buffer probes retain canonical fields.
            let compact = if !control {
                value.replacen("sha-256 ", "SHA-256 ", 1) + "\r"
            } else {
                value.clone()
            };
            memory.c_string(ptr, 256, &compact)?;
        }
        let mut remote_count = 0;
        // Exercise pre-begin buffering, then the sync marker's ignored address.
        for line in answer.sdp.lines().filter(|l| l.starts_with("a=candidate:")) {
            let fields: Vec<_> = line.split_ascii_whitespace().collect();
            if fields.get(1) != Some(&"1") {
                continue;
            }
            Candidate::from_sdp_line(line)?;
            memory.c_string(2000, 128, fields[4])?;
            instance
                .get_typed_func::<i32, ()>(&mut store, "candidate")?
                .call(&mut store, fields[5].parse()?)?;
            remote_count += 1;
        }
        if remote_count == 0 {
            bail!("server gathered no representable candidates");
        }
        instance
            .get_typed_func::<(), ()>(&mut store, "sync")?
            .call(&mut store, ())?;
        instance
            .get_typed_func::<(), ()>(&mut store, "begin")?
            .call(&mut store, ())?;
        if !control {
            let b = backend.lock().unwrap_or_else(|e| e.into_inner());
            let shape = b
                .remote_begin_diagnostic
                .as_ref()
                .context("compact credential diagnostic missing")?;
            if shape["raw"]["ufrag_terminal_cr"] != true
                || !shape["normalized"]["remote_validation_error"].is_null()
                || shape["normalized"]["parsec_padded_ufrag_compatibility"] != true
                || shape["attempt_matches"] != true
            {
                bail!("compact credential normalization was not exercised");
            }
        }
        let poll = instance.get_typed_func::<i32, i32>(&mut store, "poll")?;
        let mut gate = CandidateGate::new("test-server")?;
        gate.remote_ready("test-server", &compact.mid, &compact.credentials.ufrag)?;
        gate.sync("test-server")?;
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut local_count = 0;
        let mut ack = false;
        let mut buffer_retry = false;
        let mut connected_event = false;
        while Instant::now() < deadline && (local_count == 0 || (!dtls_failure && !ack)) {
            if !buffer_retry && poll.call(&mut store, 1).is_err() {
                buffer_retry = true;
            }
            if poll.call(&mut store, 2048)? == 0 {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            let event: serde_json::Value = serde_json::from_str(&memory.string(4096, 2048)?)?;
            if control && event["type"] == 7 && event["status"] == 0 && event["state"] == 4 {
                connected_event = true;
                continue;
            }
            if event["type"] != 8 || event["attemptID"] != "local-session-test" {
                bail!("invalid guest candidate event");
            }
            if event["sync"] == true {
                ack = true;
                continue;
            }
            let candidate = Candidate::new(
                event["ip"].as_str().context("candidate IP missing")?,
                u16::try_from(event["port"].as_u64().context("candidate port missing")?)?,
                event["fromStun"] == true,
            )?;
            gate.push("test-server", candidate)?;
            while let Some(candidate) = gate.pop_ready() {
                runtime.block_on(async {
                    tokio::time::timeout(Duration::from_secs(5), peer.add_ice_candidate(candidate))
                        .await
                })??;
                local_count += 1;
            }
        }
        if local_count == 0 || (!dtls_failure && (!ack || !buffer_retry)) {
            bail!("guest candidate exchange/sync/copy retry incomplete");
        }
        if dtls_failure {
            let mut attempt = backend
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .native_attempt
                .take()
                .context("negative native attempt absent")?;
            let until = Instant::now() + Duration::from_secs(10);
            while attempt.failure_stage().is_none() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(5));
            }
            let failure = attempt.snapshot();
            if failure["legacy_rsa_1024_enabled"] != legacy_rsa_1024
                || failure["failure_stage"] != "dtls-transport"
                || failure["transport_states_at_failure"]["dtls"] != "failed"
                || failure["transport_states_at_failure"]["peer"] != "failed"
                || failure["channels_open"] != 0
            {
                bail!(
                    "DTLS failure callback lost fresh states or allowed unauthenticated channels"
                );
            }
            if crate::transport_diagnostics::enabled() {
                // The upstream warning follows its awaited failure callback.
                // In this controlled probe only, allow that task to publish
                // before cancelling its runtime. Account behavior is unchanged.
                let until = Instant::now() + Duration::from_millis(500);
                while !crate::transport_diagnostics::contains_reason(
                    "certificate-fingerprint-mismatch",
                ) && Instant::now() < until
                {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
            attempt.cancel();
            attempt.wait_finished(Duration::from_secs(10))?;
            if attempt.snapshot()["peer_closed"] != true {
                bail!("negative native attempt did not close");
            }
            return Ok(serde_json::json!({"dtls_failure_capture_verified":true,
                "fingerprint_rejection_verified":true,"native_failure":failure,
                "scope":"controlled-native-dtls-wrong-fingerprint",
                "real_account_used":false,"external_requests_enabled":false,
                "parsec_host_connected":false,"video_decoded":false}));
        }
        let control_report = if control {
            Some(control_exchange(
                &mut store,
                &instance,
                &runtime,
                &channels[0],
                &mut rx,
                connected_event,
            )?)
        } else {
            None
        };
        let buffer_report = if buffers {
            Some(buffer_exchange(
                &mut store,
                &instance,
                &runtime,
                &channels[0],
                &mut rx,
            )?)
        } else {
            None
        };
        {
            let b = backend.lock().unwrap_or_else(|e| e.into_inner());
            b.native_attempt
                .as_ref()
                .context("active attempt absent")?
                .wait_transport(Duration::from_secs(10))?;
            let until = Instant::now() + Duration::from_secs(2);
            loop {
                let report = b.diagnostic()?;
                let active = &report["active_attempt_diagnostic"];
                if active["transport_states"]["dtls"] == "connected" {
                    if active["local_description_set"] != true
                        || active["remote_description_set"] != true
                        || active["sync_received"] != true
                        || active["transport_connected"] != true
                        || b.native_attempt.is_none()
                    {
                        bail!("live diagnostic omitted native phases or consumed the attempt");
                    }
                    if active["local_srflx_candidates"] != 0
                        || active["ice_servers_configured"] != false
                    {
                        bail!("offline fixture configured public ICE servers");
                    }
                    break;
                }
                if Instant::now() >= until {
                    bail!("native state diagnostic did not reach connected DTLS");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let mut attempt = backend
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .native_attempt
            .take()
            .context("native attempt lost")?;
        let exchange = (|| -> Result<serde_json::Value> {
            attempt.wait_transport(Duration::from_secs(10))?;
            runtime.block_on(async {
                tokio::time::timeout(Duration::from_secs(5), async {
                    while peer.connection_state() != RTCPeerConnectionState::Connected
                        || channels
                            .iter()
                            .any(|c| c.ready_state() != RTCDataChannelState::Open)
                    {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
                .await
            })?;
            if peer.connection_state() != RTCPeerConnectionState::Connected
                || channels
                    .iter()
                    .any(|c| c.ready_state() != RTCDataChannelState::Open)
            {
                bail!("test server channels not connected");
            }
            if control {
                return Ok(attempt.snapshot());
            }
            for id in 0..3u16 {
                attempt.send_binary(id, Bytes::from(vec![0xA1, id as u8, 0, 255]))?;
                runtime.block_on(async {
                    tokio::time::timeout(
                        Duration::from_secs(5),
                        channels[usize::from(id)].send(&Bytes::from(vec![0xB2, id as u8, 0, 255])),
                    )
                    .await
                })??;
            }
            let mut client_mask = 0u8;
            let mut server_mask = 0u8;
            for _ in 0..3 {
                let (id, text, data) = attempt.receive_binary(Duration::from_secs(5))?;
                if id > 2
                    || text
                    || data.as_ref() != [0xB2, id as u8, 0, 255]
                    || client_mask & (1 << id) != 0
                {
                    bail!("client binary receipt mismatch");
                }
                client_mask |= 1 << id;
                let (id, text, data) = runtime
                    .block_on(async {
                        tokio::time::timeout(Duration::from_secs(5), rx.recv()).await
                    })?
                    .context("server receipt channel closed")?;
                if id > 2
                    || text
                    || data.as_ref() != [0xA1, id as u8, 0, 255]
                    || server_mask & (1 << id) != 0
                {
                    bail!("server binary receipt mismatch");
                }
                server_mask |= 1 << id;
            }
            if backend.lock().unwrap_or_else(|e| e.into_inner()).status != Some(20) {
                bail!("transport incorrectly claimed a Parsec session");
            }
            Ok(attempt.snapshot())
        })();
        attempt.cancel();
        attempt.wait_finished(Duration::from_secs(12))?;
        let mut buffer_report = buffer_report;
        if let Some((report, key)) = &mut buffer_report {
            instance
                .get_typed_func::<(), ()>(&mut store, "disconnect")?
                .call(&mut store, ())?;
            let size = instance
                .get_typed_func::<i32, i32>(&mut store, "buffer_size")?
                .call(&mut store, *key as i32)?;
            if size != 0 {
                bail!("disconnect retained a guest buffer");
            }
            memory.write(30000, b"sentinel")?;
            instance
                .get_typed_func::<(i32, i32), ()>(&mut store, "buffer")?
                .call(&mut store, (*key as i32, 30000))?;
            if memory.read(30000, 8)? != b"sentinel" {
                bail!("stale handle wrote guest memory");
            }
            report["disconnect_cleanup_verified"] = serde_json::json!(true);
            report["stale_handle_noop_verified"] = serde_json::json!(true);
        }
        let connected = exchange?;
        let closed = attempt.snapshot();
        if closed["peer_closed"] != true || closed["failed"] != false {
            bail!("native attempt cleanup failed");
        }
        Ok(
            serde_json::json!({"schema":1,"scope":if buffers {"controlled-wasm-native-cursor-user-buffers"} else if control {"controlled-wasm-native-parsec-control"} else {"controlled-wasm-guest-native-ice-dtls-sctp"},"original_parsec_guest_attempt_exercised":false,"guest_begin_and_candidate_verified":true,"guest_local_candidate_events":local_count,"guest_sync_ack_verified":ack,"guest_buffer_retry_verified":buffer_retry,"remote_candidates_submitted":remote_count,"connected_peers":2,"binary_messages_verified":if control {0} else {6},"control":control_report,"buffers":buffer_report.map(|p|p.0),"native_connected":connected,"native_closed":closed,"parsec_host_connected":false,"video_decoded":false}),
        )
    })();
    // Tear down even when negotiation or validation fails.
    backend.lock().unwrap_or_else(|e| e.into_inner()).destroy();
    runtime
        .block_on(async { tokio::time::timeout(Duration::from_secs(5), peer.close()).await })??;
    let mut report = checked?;
    report["compact_credentials_normalization_verified"] = serde_json::json!(!control);
    report["parsec_padded_ufrag_native_negotiation_verified"] =
        serde_json::json!(!control && !dtls_failure);
    report["live_attempt_reporting_verified"] = serde_json::json!(true);
    if peer.connection_state() != RTCPeerConnectionState::Closed {
        bail!("test server did not close");
    }
    report["peers_closed"] = serde_json::json!(true);
    instance
        .get_typed_func::<(), ()>(&mut store, "destroy")?
        .call(&mut store, ())?;
    Ok(report)
}

fn control_exchange(
    store: &mut wasmtime::Store<crate::host::HostState>,
    instance: &wasmtime::Instance,
    runtime: &tokio::runtime::Runtime,
    channel: &Arc<webrtc::data_channel::RTCDataChannel>,
    receipts: &mut tokio::sync::mpsc::Receiver<(u16, bool, Bytes)>,
    mut connected_event: bool,
) -> Result<serde_json::Value> {
    let memory = store.data().memory.clone();
    let (id, is_text, startup) = runtime
        .block_on(async { tokio::time::timeout(Duration::from_secs(5), receipts.recv()).await })?
        .context("startup packet missing")?;
    if id != 0
        || is_text
        || startup.len() < 14
        || startup[12] != 11
        || startup.last() != Some(&0)
        || i32::from_be_bytes(startup[..4].try_into()?) as usize != startup.len() - 13
    {
        bail!("invalid native control startup header");
    }
    let config: serde_json::Value = serde_json::from_slice(&startup[13..startup.len() - 1])?;
    if config
        != serde_json::json!({"_version":1,"_max_w":60000,"_max_h":60000,"_flags":0,"resolutionX":1920,"resolutionY":1080,"refreshRate":60,"mediaContainer":0,"_VideoProtocolVersion":1})
    {
        bail!("native startup configuration mismatch");
    }
    let frames = [
        crate::control::header(10, 0, 0, 0),
        crate::control::header(21, 0, 12500, 0),
        crate::control::header(20, 7, 1000, 2000),
        crate::control::header(16, 1, 0, 0),
        crate::control::header(28, 3, 0, 0),
        crate::control::text(
            25,
            42,
            r#"[{"id":42,"owner":true},{"id":43,"owner":false}]"#,
        )?,
    ];
    for frame in frames {
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), channel.send(&frame)).await
        })??;
    }
    let poll = instance.get_typed_func::<i32, i32>(&mut *store, "poll")?;
    let status = instance.get_typed_func::<(), i32>(&mut *store, "status")?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut status_event = false;
    let mut rumble = false;
    let mut clipboard = false;
    while Instant::now() < deadline
        && (!connected_event
            || !status_event
            || !rumble
            || !clipboard
            || store
                .data()
                .backend
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .control_frames_received
                < 6)
    {
        status.call(&mut *store, ())?;
        if poll.call(&mut *store, 2048)? == 0 {
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }
        let event: serde_json::Value = serde_json::from_str(&memory.string(4096, 2048)?)?;
        match event["type"].as_i64() {
            Some(7) if event["status"] == 0 && event["state"] == 4 => connected_event = true,
            Some(7) if event["status"] == 0 && event["state"] == 8 => status_event = true,
            Some(2)
                if event["gamepadID"] == 7
                    && event["motorBig"] == 1000
                    && event["motorSmall"] == 2000 =>
            {
                rumble = true
            }
            Some(4) => clipboard = true,
            Some(8) => {}
            _ => bail!("unexpected control event"),
        }
    }
    if !connected_event
        || !status_event
        || !rumble
        || !clipboard
        || status.call(&mut *store, ())? != 0
    {
        bail!("control events/status not verified");
    }
    instance
        .get_typed_func::<(), ()>(&mut *store, "guests")?
        .call(&mut *store, ())?;
    let guests: serde_json::Value = serde_json::from_str(&memory.string(10000, 4096)?)?;
    if guests.as_array().map(Vec::len) != Some(2) {
        bail!("guest list bridge mismatch");
    }
    instance
        .get_typed_func::<(), i32>(&mut *store, "self")?
        .call(&mut *store, ())?;
    if memory.read(14300, 1)? != [1] || memory.u32(14304)? != 42 {
        bail!("self metadata mismatch");
    }
    if instance
        .get_typed_func::<(), i32>(&mut *store, "mode")?
        .call(&mut *store, ())?
        != 3
    {
        bail!("host mode mismatch");
    }
    instance
        .get_typed_func::<(), ()>(&mut *store, "metrics")?
        .call(&mut *store, ())?;
    if f32::from_le_bytes(memory.read(14416, 4)?.try_into().unwrap()) != 12.5 {
        bail!("encode latency mismatch");
    }
    let send = instance.get_typed_func::<(), ()>(&mut *store, "send")?;
    memory.c_string(8192, 1024, r#"{"type":1,"code":65,"mod":2,"pressed":true}"#)?;
    send.call(&mut *store, ())?;
    let (id, is_text, key) = runtime
        .block_on(async { tokio::time::timeout(Duration::from_secs(5), receipts.recv()).await })?
        .context("native key packet absent")?;
    if id != 0 || is_text || key.as_ref() != [0, 0, 0, 65, 0, 0, 0, 2, 0, 0, 0, 1, 0] {
        bail!("WASM key event binary mismatch");
    }
    memory.c_string(8192, 1024, r#"{"type":4,"relative":false,"x":1,"y":2}"#)?;
    if send.call(&mut *store, ()).is_ok() {
        bail!("unsupported absolute mouse input accepted");
    }
    Ok(
        serde_json::json!({"startup_configuration_verified":true,"wasm_input_packet_verified":true,"unsupported_absolute_mouse_rejected":true,"status_events_verified":true,"rumble_event_verified":true,"clipboard_request_event_verified":true,"guest_self_metadata_verified":true,"host_mode_verified":true,"encode_latency_verified":true,"host_frames_verified":6,"synthetic_host":true,"real_parsec_host_compatible":false}),
    )
}

fn buffer_exchange(
    store: &mut wasmtime::Store<crate::host::HostState>,
    instance: &wasmtime::Instance,
    runtime: &tokio::runtime::Runtime,
    channel: &Arc<webrtc::data_channel::RTCDataChannel>,
    receipts: &mut tokio::sync::mpsc::Receiver<(u16, bool, Bytes)>,
) -> Result<(serde_json::Value, u32)> {
    let memory = store.data().memory.clone();
    let binary = [0, 255, 16, 128, 0];
    let image = b"fixture-image";
    let mut data = crate::control::header(17, 5, 7, 0).to_vec();
    data.extend_from_slice(&binary);
    let mut cursor = vec![0; 34 + image.len()];
    cursor[12] = 9;
    cursor[16..20].copy_from_slice(&(image.len() as i32).to_be_bytes());
    for (offset, value) in [
        (20, 24i16),
        (22, 16),
        (24, -12),
        (26, 42),
        (28, 3),
        (30, 4),
        (32, 768),
    ] {
        cursor[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
    }
    cursor[34..].copy_from_slice(image);
    let mut position = cursor[..34].to_vec();
    position[16..20].copy_from_slice(&0i32.to_be_bytes());
    let frames = [
        Bytes::from(data),
        Bytes::from(cursor),
        Bytes::from(position),
        crate::control::header(17, 0, 8, 0),
    ];
    for frame in frames {
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), channel.send(&frame)).await
        })??;
    }
    let poll = instance.get_typed_func::<i32, i32>(&mut *store, "poll")?;
    let size = instance.get_typed_func::<i32, i32>(&mut *store, "buffer_size")?;
    let copy = instance.get_typed_func::<(i32, i32), ()>(&mut *store, "buffer")?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut seen = 0u8;
    let mut retained = 0;
    while seen != 15 && Instant::now() < deadline {
        if poll.call(&mut *store, 2048)? == 0 {
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }
        let event: serde_json::Value = serde_json::from_str(&memory.string(4096, 2048)?)?;
        if event["type"] == 8 {
            continue;
        }
        let key = u32::try_from(event["key"].as_u64().context("buffer event key missing")?)?;
        let (bit, expected): (u8, &[u8]) = match event["type"].as_i64() {
            Some(3) if event["id"] == 7 => (1, &binary),
            Some(3) if event["id"] == 8 => (8, &[]),
            Some(1) if event["cursor"]["imageUpdate"] == true => {
                if event["cursor"]["positionX"] != -12
                    || event["cursor"]["positionY"] != 42
                    || event["cursor"]["width"] != 24
                    || event["cursor"]["height"] != 16
                    || event["cursor"]["hotX"] != 3
                    || event["cursor"]["hotY"] != 4
                    || event["cursor"]["relative"] != true
                    || event["cursor"]["hidden"] != true
                {
                    bail!("cursor metadata mismatch");
                }
                (2, image)
            }
            Some(1)
                if event["cursor"]["imageUpdate"] == false
                    && event["cursor"]["size"] == 0
                    && key == 0 =>
            {
                (4, &[])
            }
            _ => bail!("unexpected buffer event"),
        };
        if seen & bit != 0 {
            bail!("duplicate buffer event");
        }
        seen |= bit;
        if bit == 4 {
            memory.write(20000, b"unchanged")?;
            copy.call(&mut *store, (0, 20000))?;
            if memory.read(20000, 9)? != b"unchanged" {
                bail!("no-image cursor wrote guest memory");
            }
            continue;
        }
        if key == 0 || size.call(&mut *store, key as i32)? != expected.len() as i32 {
            bail!("buffer size mismatch");
        }
        if copy.call(&mut *store, (key as i32, -1)).is_ok() {
            bail!("invalid guest destination accepted");
        }
        if size.call(&mut *store, key as i32)? != expected.len() as i32 {
            bail!("failed copy consumed buffer");
        }
        copy.call(&mut *store, (key as i32, 20000))?;
        if memory.read(20000, expected.len())? != expected
            || size.call(&mut *store, key as i32)? != 0
        {
            bail!("one-shot buffer copy mismatch");
        }
        memory.write(20000, b"unchanged")?;
        copy.call(&mut *store, (key as i32, 20000))?;
        if memory.read(20000, 9)? != b"unchanged" {
            bail!("consumed key was reused");
        }
        if bit == 1 {
            // Leave a second real incoming payload unread for disconnect cleanup.
            let mut frame = crate::control::header(17, 1, 9, 0).to_vec();
            frame.push(77);
            runtime.block_on(async {
                tokio::time::timeout(Duration::from_secs(5), channel.send(&Bytes::from(frame)))
                    .await
            })??;
        }
    }
    if seen != 15 {
        bail!("buffer exchange incomplete");
    }
    while retained == 0 && Instant::now() < deadline {
        if poll.call(&mut *store, 2048)? == 0 {
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }
        let event: serde_json::Value = serde_json::from_str(&memory.string(4096, 2048)?)?;
        if event["type"] == 8 {
            continue;
        }
        if event["type"] != 3 || event["id"] != 9 {
            bail!("cleanup payload event missing");
        }
        retained = u32::try_from(event["key"].as_u64().context("cleanup key missing")?)?;
    }
    if retained == 0 || size.call(&mut *store, retained as i32)? != 1 {
        bail!("cleanup payload not retained");
    }
    let text = "árvíz ✓";
    memory.c_string(8192, 1024, text)?;
    instance
        .get_typed_func::<i32, ()>(&mut *store, "send_user")?
        .call(&mut *store, 7)?;
    let (id, is_text, packet) = runtime
        .block_on(async { tokio::time::timeout(Duration::from_secs(5), receipts.recv()).await })?
        .context("outbound user data missing")?;
    if id != 0
        || is_text
        || packet.len() != 14 + text.len()
        || packet[12] != 17
        || i32::from_be_bytes(packet[..4].try_into()?) != text.len() as i32 + 1
        || i32::from_be_bytes(packet[4..8].try_into()?) != 7
        || &packet[13..packet.len() - 1] != text.as_bytes()
        || packet.last() != Some(&0)
    {
        bail!("Unicode user-data packet mismatch");
    }
    Ok((
        serde_json::json!({"cursor_metadata_verified":true,"cursor_payload_verified":true,"no_image_cursor_verified":true,"binary_user_data_verified":true,"empty_user_data_verified":true,"failed_copy_retry_verified":true,"one_shot_consumption_verified":true,"unicode_outbound_verified":true,"cursor_rendered":false,"clipboard_synchronized":false}),
        retained,
    ))
}

#[cfg(test)]
mod tests {
    #[test]
    fn native_dtls_failure_retains_callback_state_and_rejects_wrong_fingerprint() {
        let report = super::probe_mode(super::Mode::DtlsFailure).unwrap();
        assert_eq!(report["fingerprint_rejection_verified"], true);
        assert_eq!(report["dtls_failure_capture_verified"], true);
        assert_eq!(report["peers_closed"], true);
        assert_eq!(
            report["native_failure"]["transport_states_at_failure"]["ice"],
            "connected"
        );
    }
    #[test]
    fn invalid_remote_ufrag_retains_only_shape_and_keeps_guest_alive() {
        use wasmtime::{Config, Engine, Module};
        let mut config = Config::new();
        config
            .wasm_threads(true)
            .consume_fuel(true)
            .epoch_interruption(true);
        let engine = Engine::new(&config).unwrap();
        let module = Module::new(&engine, r#"(module
          (import "env" "memory" (memory 1 1 shared))
          (import "env" "parsec_web_init" (func $init))
          (import "env" "parsec_web_new_attempt" (func $offer (param i32 i32 i32 i32 i32 i32 i32)))
          (import "env" "parsec_web_begin_p2p" (func $begin (param i32 i32 i32 i32 i32)))
          (func (export "start") call $init
            i32.const 16 i32.const 100 i32.const 400 i32.const 700 i32.const 256 i32.const 64 i32.const 80 call $offer)
          (func (export "begin") i32.const 16 i32.const 0 i32.const 1024 i32.const 1280 i32.const 1536 call $begin))"#).unwrap();
        let (mut store, instance) = crate::instantiate(&engine, &module).unwrap();
        let memory = store.data().memory.clone();
        memory.c_string(16, 64, "ufrag-shape-test").unwrap();
        memory.c_string(1024, 258, "te==stAA").unwrap();
        memory
            .c_string(1280, 258, "abcdefghijklmnopqrstuv")
            .unwrap();
        memory
            .c_string(1536, 258, &format!("sha-256 {}", ["AB"; 32].join(":")))
            .unwrap();
        instance
            .get_typed_func::<(), ()>(&mut store, "start")
            .unwrap()
            .call(&mut store, ())
            .unwrap();
        instance
            .get_typed_func::<(), ()>(&mut store, "begin")
            .unwrap()
            .call(&mut store, ())
            .unwrap();
        let backend = store.data().backend.lock().unwrap();
        assert_eq!(backend.status, Some(-6200));
        let shape = backend.remote_begin_diagnostic.as_ref().unwrap();
        assert_eq!(shape["attempt_matches"], true);
        assert_eq!(shape["normalized"]["ufrag_shape"]["equals_bytes"], 2);
        assert_eq!(shape["normalized"]["validation_error"], "ice-ufrag");
        let report = serde_json::to_string(&*backend).unwrap();
        for secret in ["te==stAA", "abcdefghijklmnopqrstuv", "ufrag-shape-test"] {
            assert!(!report.contains(secret));
        }
        assert!(store.data().boundary.is_none());
        assert!(backend.native_attempt.is_none());
    }
    #[test]
    fn invalid_remote_candidate_returns_connection_error_without_trapping_guest() {
        use wasmtime::{Config, Engine, Module};
        let mut config = Config::new();
        config
            .wasm_threads(true)
            .consume_fuel(true)
            .epoch_interruption(true);
        let engine = Engine::new(&config).unwrap();
        let module = Module::new(&engine, r#"(module
          (import "env" "memory" (memory 1 1 shared))
          (import "env" "parsec_web_init" (func $init))
          (import "env" "parsec_web_new_attempt" (func $offer (param i32 i32 i32 i32 i32 i32 i32)))
          (import "env" "parsec_web_add_candidate" (func $candidate (param i32 i32 i32 i32 i32)))
          (func (export "start") call $init
            i32.const 16 i32.const 100 i32.const 400 i32.const 700 i32.const 256 i32.const 64 i32.const 80 call $offer)
          (func (export "candidate") i32.const 16 i32.const 1024 i32.const 1234 i32.const 0 i32.const 0 call $candidate))"#).unwrap();
        let (mut store, instance) = crate::instantiate(&engine, &module).unwrap();
        let memory = store.data().memory.clone();
        memory.c_string(16, 64, "candidate-failure-test").unwrap();
        memory
            .c_string(1024, 128, "private-sentinel.invalid")
            .unwrap();
        instance
            .get_typed_func::<(), ()>(&mut store, "start")
            .unwrap()
            .call(&mut store, ())
            .unwrap();
        let candidate = instance
            .get_typed_func::<(), ()>(&mut store, "candidate")
            .unwrap();
        candidate.call(&mut store, ()).unwrap();
        // A late candidate after cleanup must not resurrect or kill the app.
        candidate.call(&mut store, ()).unwrap();
        let backend = store.data().backend.lock().unwrap();
        assert_eq!(backend.status, Some(-6200));
        assert!(backend.native_attempt.is_none());
        assert!(backend.attempt_diagnostic.is_some());
        let report = serde_json::to_string(&*backend).unwrap();
        assert!(report.contains("candidate-address"));
        assert!(!report.contains("private-sentinel"));
        assert!(store.data().boundary.is_none());
    }
    #[test]
    fn wasm_candidate_exchange_connects_native_channels_without_claiming_parsec_session() {
        let report = super::probe().unwrap();
        assert_eq!(report["compact_credentials_normalization_verified"], true);
        assert_eq!(
            report["parsec_padded_ufrag_native_negotiation_verified"],
            true
        );
        assert_eq!(report["binary_messages_verified"], 6);
        assert_eq!(report["live_attempt_reporting_verified"], true);
        assert_eq!(report["guest_buffer_retry_verified"], true);
        assert_eq!(report["peers_closed"], true);
        assert_eq!(report["parsec_host_connected"], false);
    }
    #[test]
    fn wasm_control_imports_exchange_framed_messages_with_native_test_peer() {
        let report = super::control_probe().unwrap();
        assert_eq!(report["control"]["startup_configuration_verified"], true);
        assert_eq!(report["control"]["wasm_input_packet_verified"], true);
        assert_eq!(report["peers_closed"], true);
        assert_eq!(report["parsec_host_connected"], false);
    }
    #[test]
    fn native_cursor_and_userdata_buffers_cross_actual_wasm_imports() {
        let report = super::buffer_probe().unwrap();
        assert_eq!(report["buffers"]["failed_copy_retry_verified"], true);
        assert_eq!(report["buffers"]["one_shot_consumption_verified"], true);
        assert_eq!(report["buffers"]["disconnect_cleanup_verified"], true);
        assert_eq!(report["peers_closed"], true);
        assert_eq!(report["parsec_host_connected"], false);
    }
}
