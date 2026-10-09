//! Verify the local SCTP-only extension cannot accept RTP media SDP.
use webrtc::{
    api::{setting_engine::SettingEngine, APIBuilder},
    peer_connection::{
        configuration::RTCConfiguration, sdp::session_description::RTCSessionDescription,
        signaling_state::RTCSignalingState,
    },
};

#[tokio::test]
async fn data_only_rejects_rtp_sdp_before_state_changes() {
    let mut settings = SettingEngine::default();
    settings.set_data_channel_only(true);
    let api = APIBuilder::new().with_setting_engine(settings).build();
    let peer = api
        .new_peer_connection(RTCConfiguration::default())
        .await
        .unwrap();
    peer.create_data_channel("policy-test", None).await.unwrap();
    let offer = peer.create_offer(None).await.unwrap();
    assert!(offer.sdp.contains("m=application "));
    let media_sdp = offer.sdp.replace("m=application ", "m=audio ");
    let local = RTCSessionDescription::offer(media_sdp.clone()).unwrap();
    let remote = RTCSessionDescription::offer(media_sdp).unwrap();
    let local_result = peer.set_local_description(local).await;
    let remote_result = peer.set_remote_description(remote).await;
    let stable = peer.signaling_state() == RTCSignalingState::Stable;
    peer.close().await.unwrap();
    assert!(matches!(
        local_result,
        Err(webrtc::Error::ErrDataChannelOnlyMedia)
    ));
    assert!(matches!(
        remote_result,
        Err(webrtc::Error::ErrDataChannelOnlyMedia)
    ));
    assert!(stable);
}
