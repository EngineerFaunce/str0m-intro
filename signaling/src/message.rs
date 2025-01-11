use str0m::change::{SdpAnswer, SdpOffer};

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub enum SdpMessageType {
    SdpOffer(SdpOffer),
    SdpAnswer(SdpAnswer),
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct SdpExchange {
    pub client_id: uuid::Uuid,
    pub sdp_payload: SdpMessageType,
}
