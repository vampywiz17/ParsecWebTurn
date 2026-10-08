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
    probe_mode(false)
}

pub fn control_probe() -> Result<serde_json::Value> {
    probe_mode(true)
}

fn probe_mode(control: bool) -> Result<serde_json::Value> {
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
        let remote = Description::from_sdp(&answer.sdp)?;
        for (ptr, value) in [
            (1000, &remote.credentials.ufrag),
            (1300, &remote.credentials.password),
            (1600, &remote.credentials.fingerprint),
        ] {
            memory.c_string(ptr, 256, value)?;
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
        let poll = instance.get_typed_func::<i32, i32>(&mut store, "poll")?;
        let mut gate = CandidateGate::new("test-server")?;
        gate.remote_ready("test-server", &compact.mid, &compact.credentials.ufrag)?;
        gate.sync("test-server")?;
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut local_count = 0;
        let mut ack = false;
        let mut buffer_retry = false;
        let mut connected_event = false;
        while Instant::now() < deadline && (local_count == 0 || !ack) {
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
        if local_count == 0 || !ack || !buffer_retry {
            bail!("guest candidate exchange/sync/copy retry incomplete");
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
        let connected = exchange?;
        let closed = attempt.snapshot();
        if closed["peer_closed"] != true || closed["failed"] != false {
            bail!("native attempt cleanup failed");
        }
        Ok(
            serde_json::json!({"schema":1,"scope":if control {"controlled-wasm-native-parsec-control"} else {"controlled-wasm-guest-native-ice-dtls-sctp"},"original_parsec_guest_attempt_exercised":false,"guest_begin_and_candidate_verified":true,"guest_local_candidate_events":local_count,"guest_sync_ack_verified":ack,"guest_buffer_retry_verified":buffer_retry,"remote_candidates_submitted":remote_count,"connected_peers":2,"binary_messages_verified":if control {0} else {6},"control":control_report,"native_connected":connected,"native_closed":closed,"parsec_host_connected":false,"video_decoded":false}),
        )
    })();
    // Tear down even when negotiation or validation fails.
    backend.lock().unwrap_or_else(|e| e.into_inner()).destroy();
    runtime
        .block_on(async { tokio::time::timeout(Duration::from_secs(5), peer.close()).await })??;
    let mut report = checked?;
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

#[cfg(test)]
mod tests {
    #[test]
    fn wasm_candidate_exchange_connects_native_channels_without_claiming_parsec_session() {
        let report = super::probe().unwrap();
        assert_eq!(report["binary_messages_verified"], 6);
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
}
