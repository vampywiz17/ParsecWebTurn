//! Native data-channel transport proof, deliberately separate from host login.
//! Public webrtc-rs APIs; no browser, JS, account, STUN or TURN server involved.
pub const CHANNELS: [(u16, &str); 3] = [(0, "control"), (1, "video"), (2, "audio")];
#[cfg(any(test, feature = "diagnostics"))]
pub use diagnostics::{probe, signaling_probe};
#[cfg(any(test, feature = "diagnostics"))]
mod diagnostics {
    use super::CHANNELS;
    use crate::signaling::{Candidate, CandidateGate, Description};
    use anyhow::{bail, Context, Result};
    use bytes::Bytes;
    use serde::Serialize;
    use std::{sync::Arc, time::Duration};
    use tokio::{sync::mpsc, time::timeout};
    use webrtc::{
        api::{setting_engine::SettingEngine, APIBuilder},
        data_channel::{
            data_channel_init::RTCDataChannelInit, data_channel_message::DataChannelMessage,
            data_channel_state::RTCDataChannelState, RTCDataChannel,
        },
        ice::network_type::NetworkType,
        ice_transport::ice_gathering_state::RTCIceGatheringState,
        peer_connection::{
            configuration::RTCConfiguration, peer_connection_state::RTCPeerConnectionState,
            sdp::session_description::RTCSessionDescription, RTCPeerConnection,
        },
    };

    struct Peer {
        connection: Arc<RTCPeerConnection>,
        channels: Vec<Arc<RTCDataChannel>>,
    }

    type Receipt = (usize, u16, bool, Bytes);

    #[derive(Serialize)]
    pub struct ProbeReport {
        schema: u32,
        pub scope: &'static str,
        pub parsec_host_connected: bool,
        pub video_decoded: bool,
        pub connected_peers: u32,
        pub srtp_profiles_verified: bool,
        pub binary_messages_verified: u32,
        pub disconnect_notice_verified: bool,
        pub channels: Vec<ChannelReport>,
        pub peers_closed: bool,
        pub signaling: &'static str,
        pub compact_candidates_added: usize,
    }

    #[derive(Serialize)]
    pub struct ChannelReport {
        pub id: u16,
        pub label: &'static str,
        pub ordered: bool,
        pub negotiated: bool,
        pub bidirectional_verified: bool,
    }

    async fn peer(side: usize, received: mpsc::Sender<Receipt>) -> Result<Peer> {
        let mut settings = SettingEngine::default();
        // Use regular host candidates, not the nonstandard loopback-candidate
        // option. Both peers run on this machine; no server URLs are configured.
        settings.set_network_types(vec![NetworkType::Udp4]);
        let api = APIBuilder::new().with_setting_engine(settings).build();
        let connection = Arc::new(api.new_peer_connection(RTCConfiguration::default()).await?);
        let mut channels = Vec::new();
        for (id, label) in CHANNELS {
            let channel = match connection
                .create_data_channel(
                    label,
                    Some(RTCDataChannelInit {
                        negotiated: Some(id),
                        ordered: Some(true),
                        ..Default::default()
                    }),
                )
                .await
            {
                Ok(channel) => channel,
                Err(error) => {
                    let _ = connection.close().await;
                    return Err(error.into());
                }
            };
            let tx = received.clone();
            channel.on_message(Box::new(move |message: DataChannelMessage| {
                let tx = tx.clone();
                Box::pin(async move {
                    let _ = tx.send((side, id, message.is_string, message.data)).await;
                })
            }));
            channels.push(channel);
        }
        Ok(Peer {
            connection,
            channels,
        })
    }

    async fn exchange(
        a: &Peer,
        b: &Peer,
        rx: &mut mpsc::Receiver<Receipt>,
        compact: bool,
    ) -> Result<ProbeReport> {
        let offer = a.connection.create_offer(None).await?;
        let local = Description::from_sdp(&offer.sdp)?;
        let mut gathered = a.connection.gathering_complete_promise().await;
        a.connection.set_local_description(offer).await?;
        // This public helper signals completion by CLOSING the channel, not by
        // sending a unit value. Confirm the actual state after it wakes up.
        let _ = gathered.recv().await;
        if a.connection.ice_gathering_state() != RTCIceGatheringState::Complete {
            bail!("offer gathering did not complete");
        }
        b.connection
            .set_remote_description(
                a.connection
                    .local_description()
                    .await
                    .context("offer missing")?,
            )
            .await?;
        let mut gathered = b.connection.gathering_complete_promise().await;
        b.connection
            .set_local_description(b.connection.create_answer(None).await?)
            .await?;
        let _ = gathered.recv().await;
        if b.connection.ice_gathering_state() != RTCIceGatheringState::Complete {
            bail!("answer gathering did not complete");
        }
        let answer = b
            .connection
            .local_description()
            .await
            .context("answer missing")?;
        let mut compact_candidates_added = 0;
        if compact {
            let remote = Description::from_sdp(&answer.sdp)?;
            if remote.mid != local.mid || !answer.sdp.lines().any(|line| line == "a=setup:active") {
                bail!("test answer does not match the pinned Parsec role/MID contract");
            }
            let mut candidates = CandidateGate::new("local-test-attempt")?;
            for line in answer
                .sdp
                .lines()
                .filter(|line| line.starts_with("a=candidate:"))
            {
                // webrtc-rs's SDP serializer also emits the component-2 copy.
                // SCTP/DTLS uses one ICE component; there is no RTP/RTCP here.
                if line.split_ascii_whitespace().nth(1) == Some("2") {
                    continue;
                }
                candidates.push("local-test-attempt", Candidate::from_sdp_line(line)?)?;
            }
            // Exercise buffering before begin_p2p, then the independent sync gate.
            if candidates.pop_ready().is_some() {
                bail!("candidate released before description");
            }
            a.connection
                .set_remote_description(RTCSessionDescription::answer(
                    local.answer(&remote.credentials)?,
                )?)
                .await?;
            candidates.remote_ready("local-test-attempt", &local.mid, &remote.credentials.ufrag)?;
            if candidates.pop_ready().is_some() {
                bail!("candidate released before sync");
            }
            candidates.sync("local-test-attempt")?;
            while let Some(candidate) = candidates.pop_ready() {
                a.connection.add_ice_candidate(candidate).await?;
                compact_candidates_added += 1;
            }
            if compact_candidates_added == 0 {
                bail!("no compact UDP candidate was applied");
            }
        } else {
            a.connection.set_remote_description(answer).await?;
        }
        while a
            .channels
            .iter()
            .chain(&b.channels)
            .any(|dc| dc.ready_state() != RTCDataChannelState::Open)
        {
            if [a, b]
                .iter()
                .any(|p| p.connection.connection_state() == RTCPeerConnectionState::Failed)
            {
                bail!("native WebRTC peer failed before channels opened");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let mut expected = std::collections::BTreeMap::new();
        for (side, source) in [a, b].iter().enumerate() {
            for dc in &source.channels {
                // Synthetic binary bytes, not a claimed valid Parsec/video packet.
                let payload = Bytes::from(vec![0, 255, side as u8, dc.id() as u8, 128, 0, 1]);
                expected.insert((1 - side, dc.id()), payload.clone());
                dc.send(&payload).await?;
            }
        }
        let mut verified = 0;
        while !expected.is_empty() {
            let (side, id, is_string, bytes) = rx.recv().await.context("receipt channel closed")?;
            let sent = expected
                .remove(&(side, id))
                .context("unexpected or duplicate receipt")?;
            if is_string || bytes != sent {
                bail!("binary data-channel payload mismatch");
            }
            verified += 1;
        }
        let connected = [a, b]
            .iter()
            .filter(|p| p.connection.connection_state() == RTCPeerConnectionState::Connected)
            .count();
        if connected != 2 {
            bail!("ICE/DTLS peer connection not confirmed");
        }
        crate::attempt::send_disconnect(&a.channels[0]).await?;
        let (side, id, text, notice) = rx.recv().await.context("disconnect notice missing")?;
        if side != 1
            || id != 0
            || text
            || notice.as_ref() != [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 10]
        {
            bail!("incorrect Parsec disconnect control frame");
        }
        for peer in [a, b] {
            if peer
                .connection
                .dtls_transport()
                .negotiated_srtp_profile()
                .await
                .is_none()
            {
                bail!("default native peer lost SRTP profile negotiation");
            }
        }
        Ok(ProbeReport {
            schema: 1,
            scope: "two-native-peers-on-this-machine",
            parsec_host_connected: false,
            video_decoded: false,
            connected_peers: connected as u32,
            srtp_profiles_verified: true,
            binary_messages_verified: verified,
            disconnect_notice_verified: true,
            channels: CHANNELS
                .into_iter()
                .map(|(id, label)| ChannelReport {
                    id,
                    label,
                    ordered: true,
                    negotiated: true,
                    bidirectional_verified: true,
                })
                .collect(),
            peers_closed: false,
            signaling: if compact {
                "parsec-compact-parameters-to-standard-sdp"
            } else {
                "complete-native-sdp"
            },
            compact_candidates_added,
        })
    }

    pub fn probe() -> Result<ProbeReport> {
        run_probe(false)
    }

    pub fn signaling_probe() -> Result<ProbeReport> {
        run_probe(true)
    }

    fn run_probe(compact: bool) -> Result<ProbeReport> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        runtime.block_on(async {
            let (tx, mut rx) = mpsc::channel(16);
            let a = peer(0, tx.clone()).await?;
            let b = match peer(1, tx).await {
                Ok(peer) => peer,
                Err(error) => {
                    let _ = a.connection.close().await;
                    return Err(error);
                }
            };
            let outcome =
                timeout(Duration::from_secs(20), exchange(&a, &b, &mut rx, compact)).await;
            // Close both peers even on negotiation failure or timeout. Do not rely
            // on process termination to release UDP sockets and async tasks.
            let closed = timeout(Duration::from_secs(5), async {
                let (a_close, b_close) = tokio::join!(a.connection.close(), b.connection.close());
                a_close?;
                b_close?;
                Ok::<(), anyhow::Error>(())
            })
            .await
            .context("native peer cleanup timed out")?;
            closed?;
            let mut report = outcome.context("native transport probe timed out")??;
            report.peers_closed = a.connection.connection_state() == RTCPeerConnectionState::Closed
                && b.connection.connection_state() == RTCPeerConnectionState::Closed;
            if !report.peers_closed {
                bail!("native peers did not close");
            }
            Ok(report)
        })
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn compact_parameters_establish_real_dtls_sctp_channels() {
            let report = super::signaling_probe().unwrap();
            assert_eq!(report.binary_messages_verified, 6);
            assert_eq!(report.connected_peers, 2);
            assert!(report.compact_candidates_added > 0);
            assert!(report.peers_closed);
            assert!(!report.parsec_host_connected);
        }

        #[test]
        fn native_negotiated_channels_exchange_binary_data_and_close() {
            let report = super::probe().unwrap();
            assert_eq!(report.binary_messages_verified, 6);
            assert_eq!(report.connected_peers, 2);
            assert!(report.peers_closed);
            assert!(!report.parsec_host_connected);
            assert!(!report.video_decoded);
        }
    }
}
