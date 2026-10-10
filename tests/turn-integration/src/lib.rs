//! Separate CI fixture; never linked into the shipped client.
#[cfg(test)]
#[path = "../../../src-native/tests/turn_stream.rs"]
mod stream;

#[cfg(test)]
mod integration {
    use std::{sync::Arc, time::Duration};
    use tokio::{
        sync::mpsc,
        time::{sleep, timeout},
    };
    use webrtc::{
        api::{setting_engine::SettingEngine, APIBuilder},
        data_channel::{
            data_channel_init::RTCDataChannelInit, data_channel_state::RTCDataChannelState,
        },
        ice::network_type::NetworkType,
        ice_transport::ice_server::RTCIceServer,
        peer_connection::{
            configuration::RTCConfiguration, policy::ice_transport_policy::RTCIceTransportPolicy,
        },
    };

    async fn exchange(url: &str) {
        let mut settings = SettingEngine::default();
        settings.set_network_types(vec![NetworkType::Udp4]);
        settings.set_data_channel_only(true);
        settings.detach_data_channels();
        settings.set_sctp_max_message_size_can_send(
            webrtc::api::setting_engine::SctpMaxMessageSize::Bounded(262144),
        );
        let api = APIBuilder::new().with_setting_engine(settings).build();
        let a = api
            .new_peer_connection(RTCConfiguration {
                ice_transport_policy: RTCIceTransportPolicy::Relay,
                ice_servers: vec![RTCIceServer {
                    urls: vec![url.into()],
                    username: "fixture".into(),
                    credential: "fixture-password".into(),
                }],
                ..Default::default()
            })
            .await
            .unwrap();
        let b = api
            .new_peer_connection(RTCConfiguration::default())
            .await
            .unwrap();
        let (tx, mut rx) = mpsc::channel(8);
        let mut channels = Vec::new();
        for id in 0..3 {
            let config = Some(RTCDataChannelInit {
                negotiated: Some(id),
                ordered: Some(true),
                ..Default::default()
            });
            let ca = a
                .create_data_channel("fixture", config.clone())
                .await
                .unwrap();
            let cb = b.create_data_channel("fixture", config).await.unwrap();
            for (side, channel) in [(0, &ca), (1, &cb)] {
                let tx = tx.clone();
                let weak = Arc::downgrade(channel);
                channel.on_open(Box::new(move || {
                    let tx = tx.clone();
                    let weak = weak.clone();
                    Box::pin(async move {
                        let detached = weak.upgrade().unwrap().detach().await.unwrap();
                        tokio::spawn(async move {
                            let mut buffer = vec![0; 262144];
                            while let Ok((length, text)) =
                                detached.read_data_channel(&mut buffer).await
                            {
                                if length == 0 {
                                    break;
                                }
                                assert!(!text);
                                if tx
                                    .send((
                                        side,
                                        id,
                                        bytes::Bytes::copy_from_slice(&buffer[..length]),
                                    ))
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                            }
                        });
                    })
                }));
            }
            channels.push((ca, cb));
        }
        let mut done = a.gathering_complete_promise().await;
        a.set_local_description(a.create_offer(None).await.unwrap())
            .await
            .unwrap();
        done.recv().await;
        let offer = a.local_description().await.unwrap();
        assert!(
            offer.sdp.contains("typ relay"),
            "no relay gathered for {url}"
        );
        assert!(!offer.sdp.contains("typ host"), "fixture must force TURN");
        b.set_remote_description(offer).await.unwrap();
        let mut done = b.gathering_complete_promise().await;
        b.set_local_description(b.create_answer(None).await.unwrap())
            .await
            .unwrap();
        done.recv().await;
        a.set_remote_description(b.local_description().await.unwrap())
            .await
            .unwrap();
        while channels.iter().any(|(a, b)| {
            a.ready_state() != RTCDataChannelState::Open
                || b.ready_state() != RTCDataChannelState::Open
        }) {
            sleep(Duration::from_millis(10)).await;
        }
        // SCTP fragments a realistic video-sized message across TURN frames.
        for (id, (a, b)) in channels.iter().enumerate() {
            // The upstream SDP advertises the RFC 8841 default 64 KiB limit.
            let payload = bytes::Bytes::from(vec![id as u8; 65536]);
            a.send(&payload).await.unwrap();
            let (side, got_id, got) = rx.recv().await.unwrap();
            assert_eq!((side, got_id), (1, id as u16));
            assert_eq!(got, payload);
            b.send(&payload).await.unwrap();
            let (side, got_id, got) = rx.recv().await.unwrap();
            assert_eq!((side, got_id), (0, id as u16));
            assert_eq!(got, payload);
        }
        let _pair = a
            .sctp()
            .transport()
            .ice_transport()
            .get_selected_candidate_pair()
            .await
            .unwrap();
        // The offer above contains only relay candidates and actual bytes
        // traversed the selected pair; no private candidate-pair fields needed.
        timeout(Duration::from_secs(5), a.close())
            .await
            .unwrap()
            .unwrap();
        timeout(Duration::from_secs(5), b.close())
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn coturn_udp_tcp_and_tls_carry_real_dtls_sctp_data() {
        for url in [
            "turn:localhost:43478?transport=udp",
            "turn:localhost:43478?transport=tcp",
            "turns:localhost:43549?transport=tcp",
        ] {
            eprintln!("Verifying {url}");
            timeout(Duration::from_secs(40), exchange(url))
                .await
                .expect(url);
        }
    }

    #[tokio::test]
    async fn trusted_tls_certificate_with_wrong_hostname_is_rejected() {
        use webrtc::ice::turn_stream::TurnStream;
        // Fixture CA is trusted; its server certificate has only DNS:localhost.
        assert!(TurnStream::dial("127.0.0.1", 43549, true).await.is_err());
    }
}
