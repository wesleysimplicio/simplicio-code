//! External Loop Hub socket/pipe transport.
//!
//! This module is deliberately a client-only adapter. It attaches to an
//! already-running Unix socket or Windows named pipe, performs the versioned
//! handshake and attach exchange, and never starts a process or owns a queue.
//! The wire format is newline-delimited JSON because it is an external
//! protocol boundary; internal Code state remains governed by the binary
//! format policy.

use crate::loop_hub::{
    AdmissionReceipt, CancelRequest, HubError, HubHandshake, HubHandshakeRequest,
    HubTransport, HubTransportFactory, LifecycleReceipt, ProgressRequest, ProgressSnapshot,
    ResumeRequest, SubmitRequest, LOOP_HUB_CLIENT_SCHEMA, LOOP_HUB_PROTOCOL,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io::{self, BufRead, BufReader, Read, Write},
    sync::{Arc, Mutex},
};

const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;

/// A cursor owned by the Hub for one workflow stream.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HubCursor {
    pub workflow_id: String,
    pub after_sequence: u64,
}

/// The second half of the versioned external attach contract.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HubAttachRequest {
    pub schema: String,
    pub protocol: String,
    pub client_id: String,
    pub workspace_id: String,
    pub session_id: String,
    pub reconnect: bool,
    pub cursors: Vec<HubCursor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HubAttachReceipt {
    pub schema: String,
    pub protocol: String,
    pub hub_id: String,
    pub session_id: String,
    pub accepted: bool,
    #[serde(default)]
    pub replay_from: Vec<HubCursor>,
}

impl HubAttachReceipt {
    fn validate(&self, expected_hub_id: &str, expected_session_id: &str) -> Result<(), HubError> {
        if self.schema != LOOP_HUB_CLIENT_SCHEMA || self.protocol != LOOP_HUB_PROTOCOL {
            return Err(HubError::Incompatible(
                "Hub attach receipt uses an unsupported schema or protocol".into(),
            ));
        }
        if !self.accepted {
            return Err(HubError::Incompatible(
                "Loop Hub rejected the Code session attach".into(),
            ));
        }
        if self.hub_id != expected_hub_id || self.session_id != expected_session_id {
            return Err(HubError::Protocol(
                "Hub attach identity does not match the handshake".into(),
            ));
        }
        if self
            .replay_from
            .iter()
            .any(|cursor| cursor.workflow_id.trim().is_empty())
        {
            return Err(HubError::Protocol(
                "Hub attach returned an invalid replay cursor".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Serialize)]
struct WireRequest<'a> {
    schema: &'static str,
    id: u64,
    method: &'a str,
    payload: &'a Value,
}

#[derive(Debug, Deserialize)]
struct WireResponse {
    schema: String,
    id: u64,
    ok: bool,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<String>,
}

enum RpcFailure {
    Io(String),
    Protocol(HubError),
}

impl From<HubError> for RpcFailure {
    fn from(error: HubError) -> Self {
        Self::Protocol(error)
    }
}

trait HubReadWrite: Read + Write + Send {}
impl<T: Read + Write + Send> HubReadWrite for T {}

struct Channel {
    reader: BufReader<Box<dyn HubReadWrite>>,
    writer: Box<dyn HubReadWrite>,
}

impl Channel {
    fn connect(endpoint: &str) -> Result<Self, HubError> {
        let endpoint = endpoint.trim();
        if let Some(path) = endpoint.strip_prefix("unix://") {
            #[cfg(unix)]
            {
                let stream = std::os::unix::net::UnixStream::connect(path).map_err(io_error)?;
                let reader_stream = stream.try_clone().map_err(io_error)?;
                return Ok(Self {
                    reader: BufReader::new(Box::new(reader_stream)),
                    writer: Box::new(stream),
                });
            }
            #[cfg(not(unix))]
            {
                let _ = path;
                return Err(HubError::TransportUnavailable(
                    "unix:// endpoints are only supported on Unix".into(),
                ));
            }
        }
        if let Some(name) = endpoint.strip_prefix("pipe://") {
            #[cfg(windows)]
            {
                let path = if name.starts_with(r"\\.\pipe\") {
                    name.to_owned()
                } else {
                    format!(r"\\.\pipe\{name}")
                };
                let stream = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(path)
                    .map_err(io_error)?;
                let reader_stream = stream.try_clone().map_err(io_error)?;
                return Ok(Self {
                    reader: BufReader::new(Box::new(reader_stream)),
                    writer: Box::new(stream),
                });
            }
            #[cfg(not(windows))]
            {
                let _ = name;
                return Err(HubError::TransportUnavailable(
                    "pipe:// endpoints are only supported on Windows".into(),
                ));
            }
        }
        Err(HubError::TransportUnavailable(
            "Loop Hub endpoint must use unix:// or pipe://".into(),
        ))
    }

    fn request(&mut self, id: u64, method: &str, payload: &Value) -> Result<Value, RpcFailure> {
        let request = WireRequest {
            schema: LOOP_HUB_CLIENT_SCHEMA,
            id,
            method,
            payload,
        };
        serde_json::to_writer(&mut self.writer, &request)
            .map_err(|error| RpcFailure::Io(error.to_string()))?;
        self.writer
            .write_all(b"\n")
            .and_then(|_| self.writer.flush())
            .map_err(|error| RpcFailure::Io(error.to_string()))?;

        let mut frame = Vec::new();
        let bytes = self
            .reader
            .read_until(b'\n', &mut frame)
            .map_err(|error| RpcFailure::Io(error.to_string()))?;
        if bytes == 0 {
            return Err(RpcFailure::Io("Loop Hub closed the transport".into()));
        }
        if frame.len() > MAX_FRAME_BYTES {
            return Err(RpcFailure::Protocol(HubError::Protocol(
                "Loop Hub response exceeded the frame limit".into(),
            )));
        }
        let response: WireResponse = serde_json::from_slice(&frame).map_err(|error| {
            RpcFailure::Protocol(HubError::Protocol(format!(
                "invalid Loop Hub response: {error}"
            )))
        })?;
        if response.schema != LOOP_HUB_CLIENT_SCHEMA {
            return Err(RpcFailure::Protocol(HubError::Incompatible(
                "Loop Hub response uses an unsupported client schema".into(),
            )));
        }
        if response.id != id {
            return Err(RpcFailure::Protocol(HubError::Protocol(
                "Loop Hub response id does not match the request".into(),
            )));
        }
        if !response.ok {
            return Err(RpcFailure::Protocol(HubError::Protocol(
                response
                    .error
                    .unwrap_or_else(|| "Loop Hub rejected the request".into()),
            )));
        }
        response.result.ok_or_else(|| {
            RpcFailure::Protocol(HubError::Protocol(
                "Loop Hub response omitted result".into(),
            ))
        })
    }
}

fn io_error(error: io::Error) -> HubError {
    HubError::TransportUnavailable(error.to_string())
}

struct State {
    channel: Channel,
    next_id: u64,
    handshake_request: Option<HubHandshakeRequest>,
    handshake: Option<HubHandshake>,
    cursors: BTreeMap<String, u64>,
}

/// A concrete client transport for an already-running external Loop Hub.
///
/// Safe progress reads may be replayed after a reconnect using the same
/// `after_sequence`. Submit, cancel, and resume are never retried after a
/// broken connection because their receipt may be unknown; the client first
/// re-attaches and then returns a fail-closed transport error.
pub struct SocketPipeHubTransport {
    endpoint: String,
    state: Mutex<State>,
}

impl SocketPipeHubTransport {
    pub fn connect(endpoint: &str) -> Result<Self, HubError> {
        let endpoint = endpoint.trim().to_owned();
        if endpoint.is_empty() {
            return Err(HubError::EndpointNotFound);
        }
        Ok(Self {
            state: Mutex::new(State {
                channel: Channel::connect(&endpoint)?,
                next_id: 1,
                handshake_request: None,
                handshake: None,
                cursors: BTreeMap::new(),
            }),
            endpoint,
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    fn state(&self) -> Result<std::sync::MutexGuard<'_, State>, HubError> {
        self.state
            .lock()
            .map_err(|_| HubError::Protocol("Loop Hub transport lock poisoned".into()))
    }

    fn rpc_once(state: &mut State, method: &str, payload: &Value) -> Result<Value, RpcFailure> {
        let id = state.next_id;
        state.next_id = state.next_id.saturating_add(1);
        state.channel.request(id, method, payload)
    }

    fn attach_locked(
        &self,
        state: &mut State,
        request: &HubHandshakeRequest,
        reconnect: bool,
    ) -> Result<(), HubError> {
        let cursors = state
            .cursors
            .iter()
            .map(|(workflow_id, after_sequence)| HubCursor {
                workflow_id: workflow_id.clone(),
                after_sequence: *after_sequence,
            })
            .collect();
        let payload = serde_json::to_value(HubAttachRequest {
            schema: LOOP_HUB_CLIENT_SCHEMA.into(),
            protocol: LOOP_HUB_PROTOCOL.into(),
            client_id: request.client_id.clone(),
            workspace_id: request.workspace_id.clone(),
            session_id: request.session_id.clone(),
            reconnect,
            cursors,
        })
        .map_err(|error| HubError::Protocol(error.to_string()))?;
        let value = Self::rpc_once(state, "attach", &payload).map_err(rpc_to_hub_error)?;
        let receipt: HubAttachReceipt = serde_json::from_value(value)
            .map_err(|error| HubError::Protocol(format!("invalid Hub attach receipt: {error}")))?;
        let handshake = state.handshake.as_ref().ok_or_else(|| {
            HubError::Protocol("Hub attach attempted before handshake".into())
        })?;
        receipt.validate(&handshake.hub_id, &request.session_id)
    }

    fn reconnect_locked(&self, state: &mut State) -> Result<(), HubError> {
        let request = state.handshake_request.clone().ok_or_else(|| {
            HubError::TransportUnavailable("transport has not completed its handshake".into())
        })?;
        state.channel = Channel::connect(&self.endpoint)?;
        state.handshake = None;
        let payload = serde_json::to_value(&request)
            .map_err(|error| HubError::Protocol(error.to_string()))?;
        let value = Self::rpc_once(state, "handshake", &payload).map_err(rpc_to_hub_error)?;
        let handshake: HubHandshake = serde_json::from_value(value)
            .map_err(|error| HubError::Protocol(format!("invalid Hub handshake: {error}")))?;
        handshake.validate()?;
        state.handshake = Some(handshake);
        self.attach_locked(state, &request, true)
    }

    fn rpc(
        &self,
        method: &str,
        payload: &Value,
        replay_safe: bool,
    ) -> Result<Value, HubError> {
        let mut state = self.state()?;
        match Self::rpc_once(&mut state, method, payload) {
            Ok(value) => Ok(value),
            Err(RpcFailure::Protocol(error)) => Err(error),
            Err(RpcFailure::Io(first_error)) => {
                let reconnect = self.reconnect_locked(&mut state);
                if replay_safe {
                    reconnect?;
                    Self::rpc_once(&mut state, method, payload).map_err(rpc_to_hub_error)
                } else {
                    match reconnect {
                        Ok(()) => Err(HubError::TransportUnavailable(format!(
                            "Loop Hub request outcome is unknown after reconnect: {first_error}"
                        ))),
                        Err(error) => Err(HubError::TransportUnavailable(format!(
                            "Loop Hub request failed and reattach failed: {first_error}; {error}"
                        ))),
                    }
                }
            }
        }
    }

    fn value<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        payload: &Value,
        replay_safe: bool,
    ) -> Result<T, HubError> {
        let value = self.rpc(method, payload, replay_safe)?;
        serde_json::from_value(value)
            .map_err(|error| HubError::Protocol(format!("invalid {method} response: {error}")))
    }
}

fn rpc_to_hub_error(error: RpcFailure) -> HubError {
    match error {
        RpcFailure::Io(error) => HubError::TransportUnavailable(error),
        RpcFailure::Protocol(error) => error,
    }
}

impl HubTransport for SocketPipeHubTransport {
    fn handshake(&self, request: &HubHandshakeRequest) -> Result<HubHandshake, HubError> {
        let mut state = self.state()?;
        if let Some(existing) = &state.handshake_request {
            if existing != request {
                return Err(HubError::InvalidRequest(
                    "a transport cannot attach two different Code sessions".into(),
                ));
            }
            return state
                .handshake
                .clone()
                .ok_or_else(|| HubError::Protocol("missing completed Hub handshake".into()));
        }
        let payload = serde_json::to_value(request)
            .map_err(|error| HubError::Protocol(error.to_string()))?;
        let value = Self::rpc_once(&mut state, "handshake", &payload).map_err(rpc_to_hub_error)?;
        let handshake: HubHandshake = serde_json::from_value(value)
            .map_err(|error| HubError::Protocol(format!("invalid Hub handshake: {error}")))?;
        handshake.validate()?;
        state.handshake_request = Some(request.clone());
        state.handshake = Some(handshake.clone());
        let attach_result = self.attach_locked(&mut state, request, false);
        if let Err(error) = attach_result {
            state.handshake_request = None;
            state.handshake = None;
            return Err(error);
        }
        Ok(handshake)
    }

    fn submit(&self, request: &SubmitRequest) -> Result<AdmissionReceipt, HubError> {
        self.value(
            "submit",
            &serde_json::to_value(request).map_err(|error| HubError::Protocol(error.to_string()))?,
            false,
        )
    }

    fn progress(&self, request: &ProgressRequest) -> Result<ProgressSnapshot, HubError> {
        let snapshot: ProgressSnapshot = self.value(
            "progress",
            &serde_json::to_value(request).map_err(|error| HubError::Protocol(error.to_string()))?,
            true,
        )?;
        if snapshot.workflow_id != request.workflow_id || snapshot.next_sequence < request.after_sequence
        {
            return Err(HubError::Protocol(
                "Hub returned a progress snapshot with an invalid cursor".into(),
            ));
        }
        let mut state = self.state()?;
        state
            .cursors
            .insert(request.workflow_id.clone(), snapshot.next_sequence);
        Ok(snapshot)
    }

    fn cancel(&self, request: &CancelRequest) -> Result<LifecycleReceipt, HubError> {
        self.value(
            "cancel",
            &serde_json::to_value(request).map_err(|error| HubError::Protocol(error.to_string()))?,
            false,
        )
    }

    fn resume(&self, request: &ResumeRequest) -> Result<LifecycleReceipt, HubError> {
        self.value(
            "resume",
            &serde_json::to_value(request).map_err(|error| HubError::Protocol(error.to_string()))?,
            false,
        )
    }
}

/// Factory for the standard external Unix socket/named pipe transport.
#[derive(Debug, Clone, Copy, Default)]
pub struct SocketPipeHubTransportFactory;

impl HubTransportFactory for SocketPipeHubTransportFactory {
    fn connect(&self, endpoint: &str) -> Result<Arc<dyn HubTransport>, HubError> {
        Ok(Arc::new(SocketPipeHubTransport::connect(endpoint)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loop_hub::{HubMode, LoopHubClient};
    use serde_json::json;
    use std::{
        io::{BufRead, BufReader, Write},
        thread,
        time::{SystemTime, UNIX_EPOCH},
    };
    #[cfg(unix)]
    use std::os::unix::net::UnixStream;

    #[test]
    fn attach_request_is_versioned_and_carries_reconnect_cursors() {
        let request = HubAttachRequest {
            schema: LOOP_HUB_CLIENT_SCHEMA.into(),
            protocol: LOOP_HUB_PROTOCOL.into(),
            client_id: "code".into(),
            workspace_id: "workspace".into(),
            session_id: "session".into(),
            reconnect: true,
            cursors: vec![HubCursor {
                workflow_id: "workflow".into(),
                after_sequence: 7,
            }],
        };
        let encoded = serde_json::to_value(&request).unwrap();
        assert_eq!(encoded["schema"], LOOP_HUB_CLIENT_SCHEMA);
        assert_eq!(encoded["protocol"], LOOP_HUB_PROTOCOL);
        assert_eq!(encoded["reconnect"], true);
        assert_eq!(encoded["cursors"][0]["after_sequence"], 7);
    }

    #[test]
    fn unsupported_endpoint_fails_closed_without_spawning() {
        let config = HubClientConfigForTest::required();
        let result = LoopHubClient::connect(config, &SocketPipeHubTransportFactory);
        assert!(matches!(result, Err(HubError::TransportUnavailable(_))));
    }

    #[cfg(unix)]
    #[test]
    fn unix_socket_reconnects_with_the_last_progress_cursor() {
        use std::os::unix::net::{UnixListener, UnixStream};

        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("simplicio-loop-hub-{suffix}.sock"));
        let listener = UnixListener::bind(&path).unwrap();
        let server_path = path.clone();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            expect_method(&mut reader, &mut writer, "handshake", json!({
                "schema": LOOP_HUB_CLIENT_SCHEMA,
                "protocol": LOOP_HUB_PROTOCOL,
                "hub_id": "hub-1",
                "ready": true,
                "services": [
                    {"name":"runtime","owner":"loop-hub","process_id":"runtime"},
                    {"name":"mapper","owner":"loop-hub","process_id":"mapper"},
                    {"name":"scheduler","owner":"loop-hub","process_id":"scheduler"},
                    {"name":"inference","owner":"loop-hub","process_id":"inference"}
                ],
                "resources": {
                    "runtime":{"id":"runtime","capacity":1,"used":0},
                    "mapper":{"id":"mapper","capacity":1,"used":0},
                    "inference":{"id":"inference","capacity":1,"used":0},
                    "max_active_inference":1,
                    "interactive_reserved":1
                },
                "queue":{"interactive_capacity":1,"background_capacity":1,"max_pending_interactive":2},
                "local_scheduler":false
            }));
            expect_method(&mut reader, &mut writer, "attach", json!({
                "schema": LOOP_HUB_CLIENT_SCHEMA,
                "protocol": LOOP_HUB_PROTOCOL,
                "hub_id":"hub-1",
                "session_id":"session",
                "accepted":true,
                "replay_from":[]
            }));
            expect_method(&mut reader, &mut writer, "submit", json!({
                "schema": LOOP_HUB_CLIENT_SCHEMA,
                "workflow_id":"workflow",
                "state":"queued",
                "queue_position":1,
                "retry_after_ms":null,
                "receipt_id":"receipt"
            }));
            expect_method(&mut reader, &mut writer, "progress", json!({
                "workflow_id":"workflow",
                "next_sequence":1,
                "events":[{"type":"queued","sequence":0,"position":1}],
                "terminal":false
            }));
            drop(writer);

            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            expect_method(&mut reader, &mut writer, "handshake", json!({
                "schema": LOOP_HUB_CLIENT_SCHEMA,
                "protocol": LOOP_HUB_PROTOCOL,
                "hub_id": "hub-1",
                "ready": true,
                "services": [
                    {"name":"runtime","owner":"loop-hub","process_id":"runtime"},
                    {"name":"mapper","owner":"loop-hub","process_id":"mapper"},
                    {"name":"scheduler","owner":"loop-hub","process_id":"scheduler"},
                    {"name":"inference","owner":"loop-hub","process_id":"inference"}
                ],
                "resources": {
                    "runtime":{"id":"runtime","capacity":1,"used":0},
                    "mapper":{"id":"mapper","capacity":1,"used":0},
                    "inference":{"id":"inference","capacity":1,"used":0},
                    "max_active_inference":1,
                    "interactive_reserved":1
                },
                "queue":{"interactive_capacity":1,"background_capacity":1,"max_pending_interactive":2},
                "local_scheduler":false
            }));
            let request = read_request(&mut reader);
            assert_eq!(request["method"], "attach");
            assert_eq!(request["payload"]["reconnect"], true);
            assert_eq!(request["payload"]["cursors"][0]["workflow_id"], "workflow");
            assert_eq!(request["payload"]["cursors"][0]["after_sequence"], 1);
            write_response(&mut writer, request["id"].as_u64().unwrap(), json!({
                "schema": LOOP_HUB_CLIENT_SCHEMA,
                "protocol": LOOP_HUB_PROTOCOL,
                "hub_id":"hub-1",
                "session_id":"session",
                "accepted":true,
                "replay_from":[{"workflow_id":"workflow","after_sequence":1}]
            }));
            expect_method(&mut reader, &mut writer, "progress", json!({
                "workflow_id":"workflow",
                "next_sequence":2,
                "events":[{"type":"output","sequence":1,"text":"replayed"}],
                "terminal":false
            }));
            std::fs::remove_file(server_path).ok();
        });

        let mut config = crate::loop_hub::HubClientConfig::new(
            HubMode::Required,
            "code",
            "workspace",
            "session",
        );
        config.endpoint = Some(format!("unix://{}", path.display()));
        let client = LoopHubClient::connect(config, &SocketPipeHubTransportFactory)
            .unwrap()
            .unwrap();
        let mut job = client
            .submit_interactive(crate::loop_hub::InteractiveGoal::new(
                "goal",
                "turn",
                Value::Null,
            ))
            .unwrap();
        assert_eq!(job.poll().unwrap().next_sequence, 1);
        assert_eq!(job.poll().unwrap().next_sequence, 2);
        server.join().unwrap();
    }

    #[cfg(unix)]
    fn read_request(reader: &mut BufReader<UnixStream>) -> Value {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    #[cfg(unix)]
    fn write_response(writer: &mut UnixStream, id: u64, result: Value) {
        serde_json::to_writer(
            &mut *writer,
            &json!({"schema":LOOP_HUB_CLIENT_SCHEMA,"id":id,"ok":true,"result":result}),
        )
        .unwrap();
        writer.write_all(b"\n").unwrap();
        writer.flush().unwrap();
    }

    #[cfg(unix)]
    fn expect_method(reader: &mut BufReader<UnixStream>, writer: &mut UnixStream, method: &str, result: Value) {
        let request = read_request(reader);
        assert_eq!(request["method"], method);
        write_response(writer, request["id"].as_u64().unwrap(), result);
    }

    struct HubClientConfigForTest;
    impl HubClientConfigForTest {
        fn required() -> crate::loop_hub::HubClientConfig {
            let mut config = crate::loop_hub::HubClientConfig::new(
                HubMode::Required,
                "code",
                "workspace",
                "session",
            );
            config.endpoint = Some("unsupported://local".into());
            config
        }
    }
}
