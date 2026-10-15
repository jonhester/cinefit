//! The media remote protocol's messages: only the fields read or sent here.
//!
//! Field numbers are from pyatv's `.proto` files (MIT; `pyatv/protocols/mrp/
//! protobuf/`). A `ProtocolMessage` carries its inner message as a proto2
//! extension, which on the wire is an ordinary field, so each one used is a
//! plain optional field here. Enums are kept as integers: a value this code
//! does not know must not fail the whole message.

use prost::Message;

pub const SET_STATE: i32 = 4;
pub const DEVICE_INFO: i32 = 15;
pub const CLIENT_UPDATES_CONFIG: i32 = 16;
pub const GET_KEYBOARD_SESSION: i32 = 24;
pub const SET_CONNECTION_STATE: i32 = 38;
pub const SET_NOW_PLAYING_CLIENT: i32 = 46;
pub const UPDATE_CLIENT: i32 = 55;
pub const UPDATE_CONTENT_ITEM: i32 = 56;

#[derive(Clone, PartialEq, Message)]
pub struct ProtocolMessage {
    #[prost(int32, optional, tag = "1")]
    pub r#type: Option<i32>,
    #[prost(string, optional, tag = "2")]
    pub identifier: Option<String>,
    #[prost(int32, optional, tag = "4")]
    pub error_code: Option<i32>,
    #[prost(string, optional, tag = "85")]
    pub unique_identifier: Option<String>,
    #[prost(message, optional, tag = "9")]
    pub set_state: Option<SetState>,
    #[prost(message, optional, tag = "20")]
    pub device_info: Option<DeviceInfo>,
    #[prost(message, optional, tag = "21")]
    pub client_updates_config: Option<ClientUpdatesConfig>,
    #[prost(message, optional, tag = "42")]
    pub set_connection_state: Option<SetConnectionState>,
    #[prost(message, optional, tag = "50")]
    pub set_now_playing_client: Option<SetNowPlayingClient>,
    #[prost(message, optional, tag = "60")]
    pub update_content_item: Option<UpdateContentItem>,
}

#[derive(Clone, PartialEq, Message)]
pub struct DeviceInfo {
    #[prost(string, optional, tag = "1")]
    pub unique_identifier: Option<String>,
    #[prost(string, optional, tag = "2")]
    pub name: Option<String>,
    #[prost(string, optional, tag = "3")]
    pub localized_model_name: Option<String>,
    #[prost(string, optional, tag = "4")]
    pub system_build_version: Option<String>,
    #[prost(string, optional, tag = "5")]
    pub application_bundle_identifier: Option<String>,
    #[prost(string, optional, tag = "6")]
    pub application_bundle_version: Option<String>,
    #[prost(int32, optional, tag = "7")]
    pub protocol_version: Option<i32>,
    #[prost(uint32, optional, tag = "8")]
    pub last_supported_message_type: Option<u32>,
    #[prost(bool, optional, tag = "9")]
    pub supports_system_pairing: Option<bool>,
    #[prost(bool, optional, tag = "10")]
    pub allows_pairing: Option<bool>,
    #[prost(string, optional, tag = "12")]
    pub system_media_application: Option<String>,
    #[prost(bool, optional, tag = "13")]
    pub supports_acl: Option<bool>,
    #[prost(bool, optional, tag = "14")]
    pub supports_shared_queue: Option<bool>,
    #[prost(bool, optional, tag = "15")]
    pub supports_extended_motion: Option<bool>,
    #[prost(uint32, optional, tag = "17")]
    pub shared_queue_version: Option<u32>,
    #[prost(int32, optional, tag = "21")]
    pub device_class: Option<i32>,
    #[prost(uint32, optional, tag = "22")]
    pub logical_device_count: Option<u32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ClientUpdatesConfig {
    #[prost(bool, optional, tag = "1")]
    pub artwork_updates: Option<bool>,
    #[prost(bool, optional, tag = "2")]
    pub now_playing_updates: Option<bool>,
    #[prost(bool, optional, tag = "3")]
    pub volume_updates: Option<bool>,
    #[prost(bool, optional, tag = "4")]
    pub keyboard_updates: Option<bool>,
    #[prost(bool, optional, tag = "5")]
    pub output_device_updates: Option<bool>,
}

#[derive(Clone, PartialEq, Message)]
pub struct SetConnectionState {
    /// 2 is connected.
    #[prost(int32, optional, tag = "1")]
    pub state: Option<i32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct SetState {
    #[prost(message, optional, tag = "1")]
    pub now_playing_info: Option<NowPlayingInfo>,
    #[prost(message, optional, tag = "3")]
    pub playback_queue: Option<PlaybackQueue>,
    #[prost(string, optional, tag = "4")]
    pub display_id: Option<String>,
    #[prost(string, optional, tag = "5")]
    pub display_name: Option<String>,
    /// 1 playing, 2 paused, 3 stopped, 4 interrupted, 5 seeking.
    #[prost(int32, optional, tag = "6")]
    pub playback_state: Option<i32>,
    #[prost(message, optional, tag = "9")]
    pub player_path: Option<PlayerPath>,
    #[prost(double, optional, tag = "11")]
    pub playback_state_timestamp: Option<f64>,
}

#[derive(Clone, PartialEq, Message)]
pub struct NowPlayingInfo {
    #[prost(double, optional, tag = "3")]
    pub duration: Option<f64>,
    #[prost(double, optional, tag = "4")]
    pub elapsed_time: Option<f64>,
    #[prost(float, optional, tag = "5")]
    pub playback_rate: Option<f32>,
    #[prost(double, optional, tag = "8")]
    pub timestamp: Option<f64>,
    #[prost(string, optional, tag = "9")]
    pub title: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
pub struct PlaybackQueue {
    #[prost(int32, optional, tag = "1")]
    pub location: Option<i32>,
    #[prost(message, repeated, tag = "2")]
    pub content_items: Vec<ContentItem>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ContentItem {
    #[prost(string, optional, tag = "1")]
    pub identifier: Option<String>,
    #[prost(message, optional, tag = "2")]
    pub metadata: Option<ContentItemMetadata>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ContentItemMetadata {
    #[prost(string, optional, tag = "1")]
    pub title: Option<String>,
    #[prost(int32, optional, tag = "10")]
    pub season_number: Option<i32>,
    #[prost(int32, optional, tag = "11")]
    pub episode_number: Option<i32>,
    #[prost(double, optional, tag = "14")]
    pub duration: Option<f64>,
    #[prost(double, optional, tag = "35")]
    pub elapsed_time: Option<f64>,
    #[prost(float, optional, tag = "39")]
    pub playback_rate: Option<f32>,
    #[prost(string, optional, tag = "44")]
    pub content_identifier: Option<String>,
    #[prost(int64, optional, tag = "54")]
    pub itunes_store_identifier: Option<i64>,
    #[prost(string, optional, tag = "63")]
    pub series_name: Option<String>,
    /// 1 audio, 2 video.
    #[prost(int32, optional, tag = "64")]
    pub media_type: Option<i32>,
    #[prost(double, optional, tag = "74")]
    pub elapsed_time_timestamp: Option<f64>,
}

#[derive(Clone, PartialEq, Message)]
pub struct PlayerPath {
    #[prost(message, optional, tag = "2")]
    pub client: Option<NowPlayingClient>,
}

#[derive(Clone, PartialEq, Message)]
pub struct NowPlayingClient {
    #[prost(string, optional, tag = "2")]
    pub bundle_identifier: Option<String>,
    #[prost(string, optional, tag = "7")]
    pub display_name: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
pub struct SetNowPlayingClient {
    #[prost(message, optional, tag = "1")]
    pub client: Option<NowPlayingClient>,
}

#[derive(Clone, PartialEq, Message)]
pub struct UpdateContentItem {
    #[prost(message, repeated, tag = "1")]
    pub content_items: Vec<ContentItem>,
    #[prost(message, optional, tag = "2")]
    pub player_path: Option<PlayerPath>,
}

/// A message of `kind` with a fresh unique identifier, as pyatv's
/// `messages.create`.
pub fn message(kind: i32) -> ProtocolMessage {
    ProtocolMessage {
        r#type: Some(kind),
        error_code: Some(0),
        unique_identifier: Some(crate::uuid()),
        ..Default::default()
    }
}

/// Who we say we are: pyatv's values (`messages.device_information`), a
/// remote app on a phone, which is what the Apple TV expects to talk to.
pub fn device_info(client_id: &str, name: &str) -> ProtocolMessage {
    ProtocolMessage {
        identifier: Some(crate::uuid()),
        device_info: Some(DeviceInfo {
            unique_identifier: Some(client_id.to_string()),
            name: Some(name.to_string()),
            localized_model_name: Some("iPhone".into()),
            system_build_version: Some("18G82".into()),
            application_bundle_identifier: Some("com.apple.TVRemote".into()),
            application_bundle_version: Some("344.28".into()),
            protocol_version: Some(1),
            last_supported_message_type: Some(108),
            supports_system_pairing: Some(true),
            allows_pairing: Some(true),
            system_media_application: Some("com.apple.TVMusic".into()),
            supports_acl: Some(true),
            supports_shared_queue: Some(true),
            supports_extended_motion: Some(true),
            shared_queue_version: Some(2),
            device_class: Some(1),
            logical_device_count: Some(1),
        }),
        ..message(DEVICE_INFO)
    }
}

pub fn set_connection_state() -> ProtocolMessage {
    ProtocolMessage {
        set_connection_state: Some(SetConnectionState { state: Some(2) }),
        ..message(SET_CONNECTION_STATE)
    }
}

/// As pyatv asks: now-playing state arrives as set-state messages whatever
/// `now_playing_updates` says.
pub fn client_updates_config() -> ProtocolMessage {
    ProtocolMessage {
        identifier: Some(crate::uuid()),
        client_updates_config: Some(ClientUpdatesConfig {
            artwork_updates: Some(true),
            now_playing_updates: Some(false),
            volume_updates: Some(true),
            keyboard_updates: Some(true),
            output_device_updates: Some(true),
        }),
        ..message(CLIENT_UPDATES_CONFIG)
    }
}

pub fn get_keyboard_session() -> ProtocolMessage {
    ProtocolMessage {
        identifier: Some(crate::uuid()),
        ..message(GET_KEYBOARD_SESSION)
    }
}
