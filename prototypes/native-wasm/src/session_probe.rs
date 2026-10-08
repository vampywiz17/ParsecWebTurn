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
      (data (i32.const 16) "local-session-test\00")
      (func (export "offer") (result i32)
        call $init
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
        .get_typed_func::<(), i32>(&mut store, "offer")?
        .call(&mut store, ())?
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
        while Instant::now() < deadline && (local_count == 0 || !ack) {
            if !buffer_retry && poll.call(&mut store, 1).is_err() {
                buffer_retry = true;
            }
            if poll.call(&mut store, 2048)? == 0 {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            let event: serde_json::Value = serde_json::from_str(&memory.string(4096, 2048)?)?;
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
        let mut attempt = backend
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .native_attempt
            .take()
            .context("native attempt lost")?;
        let exchange = (|| -> Result<serde_json::Value> {
            attempt.wait_transport(Duration::from_secs(10))?;
            if peer.connection_state() != RTCPeerConnectionState::Connected
                || channels
                    .iter()
                    .any(|c| c.ready_state() != RTCDataChannelState::Open)
            {
                bail!("test server channels not connected");
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
            serde_json::json!({"schema":1,"scope":"controlled-wasm-guest-native-ice-dtls-sctp","original_parsec_guest_attempt_exercised":false,"guest_begin_and_candidate_verified":true,"guest_local_candidate_events":local_count,"guest_sync_ack_verified":ack,"guest_buffer_retry_verified":buffer_retry,"remote_candidates_submitted":remote_count,"connected_peers":2,"binary_messages_verified":6,"native_connected":connected,"native_closed":closed,"parsec_host_connected":false,"video_decoded":false}),
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
}
