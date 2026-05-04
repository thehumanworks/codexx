use std::sync::OnceLock;

use serde::Serialize;

const PROTO_SOURCE: &str = include_str!("../../proto/codex.app_server.v2.proto");

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RpcService {
    ClientToServer,
    ServerToClient,
    ClientNotifications,
    ServerNotifications,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcDescriptor {
    pub service: RpcService,
    pub variant: String,
    pub jsonrpc_method: String,
    pub params_type: String,
    pub response_type: String,
    pub experimental_reason: Option<String>,
}

#[derive(Debug)]
struct ProtoRegistry {
    rpcs: Vec<RpcDescriptor>,
}

static REGISTRY: OnceLock<ProtoRegistry> = OnceLock::new();

pub fn descriptors(service: RpcService) -> impl Iterator<Item = &'static RpcDescriptor> {
    registry()
        .rpcs
        .iter()
        .filter(move |descriptor| descriptor.service == service)
}

pub fn all_descriptors() -> impl Iterator<Item = &'static RpcDescriptor> {
    registry().rpcs.iter()
}

pub fn descriptor_by_method(
    service: RpcService,
    jsonrpc_method: &str,
) -> Option<&'static RpcDescriptor> {
    descriptors(service).find(|descriptor| descriptor.jsonrpc_method == jsonrpc_method)
}

pub fn descriptor_by_variant(service: RpcService, variant: &str) -> Option<&'static RpcDescriptor> {
    descriptors(service).find(|descriptor| descriptor.variant == variant)
}

pub fn proto_source() -> &'static str {
    PROTO_SOURCE
}

fn registry() -> &'static ProtoRegistry {
    REGISTRY.get_or_init(|| ProtoRegistry {
        rpcs: parse_proto_registry(PROTO_SOURCE),
    })
}

fn parse_proto_registry(source: &str) -> Vec<RpcDescriptor> {
    let mut service = None;
    let mut current: Option<RpcDescriptor> = None;
    let mut descriptors = Vec::new();

    for raw_line in source.lines() {
        let line = raw_line.trim();
        match line {
            "service ClientToServer {" => {
                service = Some(RpcService::ClientToServer);
                continue;
            }
            "service ServerToClient {" => {
                service = Some(RpcService::ServerToClient);
                continue;
            }
            "service ClientNotifications {" => {
                service = Some(RpcService::ClientNotifications);
                continue;
            }
            "service ServerNotifications {" => {
                service = Some(RpcService::ServerNotifications);
                continue;
            }
            "}" if current.is_none() => {
                service = None;
                continue;
            }
            _ => {}
        }

        if let Some(descriptor) = current.as_mut() {
            if let Some(value) = option_value(line, "jsonrpc_method") {
                descriptor.jsonrpc_method = value;
                continue;
            }
            if let Some(value) = option_value(line, "rust_variant") {
                descriptor.variant = value;
                continue;
            }
            if let Some(value) = option_value(line, "experimental_reason") {
                descriptor.experimental_reason = Some(value);
                continue;
            }
            if line == "}" {
                if let Some(descriptor) = current.take() {
                    descriptors.push(descriptor);
                }
                continue;
            }
        }

        let Some(service) = service else {
            continue;
        };
        let Some(rpc) = parse_rpc_header(service, line) else {
            continue;
        };
        current = Some(rpc);
    }

    descriptors
}

fn parse_rpc_header(service: RpcService, line: &str) -> Option<RpcDescriptor> {
    let rest = line.strip_prefix("rpc ")?;
    let (variant, rest) = rest.split_once('(')?;
    let (params_type, rest) = rest.split_once(") returns (")?;
    let (response_type, _) = rest.split_once(')')?;
    Some(RpcDescriptor {
        service,
        variant: variant.to_string(),
        jsonrpc_method: String::new(),
        params_type: params_type.to_string(),
        response_type: response_type.to_string(),
        experimental_reason: None,
    })
}

fn option_value(line: &str, name: &str) -> Option<String> {
    let option_prefix = format!("option ({name}) = \"");
    let value = line.strip_prefix(&option_prefix)?;
    let value = value.strip_suffix("\";")?;
    Some(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn parses_proto_registry_services() {
        assert_eq!(descriptors(RpcService::ClientToServer).count(), 98);
        assert_eq!(descriptors(RpcService::ServerToClient).count(), 9);
        assert_eq!(descriptors(RpcService::ClientNotifications).count(), 1);
        assert_eq!(descriptors(RpcService::ServerNotifications).count(), 62);

        let thread_start =
            descriptor_by_method(RpcService::ClientToServer, "thread/start").unwrap();
        assert_eq!(thread_start.variant, "ThreadStart");
        assert_eq!(thread_start.params_type, "ThreadStartParams");
        assert_eq!(thread_start.response_type, "ThreadStartResponse");
    }
}
