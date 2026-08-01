use spooky_config::config::{ControlApi as ControlApiConfig, ControlApiAuditFormat, ControlApiAuditSink};

#[derive(Debug, Clone, Eq, PartialEq)]
pub(in crate::quic_listener) struct ControlApiAdminAuditEmitter {
    pub(in crate::quic_listener) enabled: bool,
    pub(in crate::quic_listener) format: ControlApiAuditFormat,
    pub(in crate::quic_listener) sink: ControlApiAdminAuditTarget,
}

impl ControlApiAdminAuditEmitter {
    pub(in crate::quic_listener) fn from_config(config: &ControlApiConfig) -> Self {
        Self {
            enabled: config.audit.enabled,
            format: config.audit.format,
            sink: match config.audit.sink {
                ControlApiAuditSink::Log => ControlApiAdminAuditTarget::Log,
                ControlApiAuditSink::File => {
                    ControlApiAdminAuditTarget::File(config.audit.file_path.clone())
                }
            },
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(in crate::quic_listener) enum ControlApiAdminAuditTarget {
    Log,
    File(Option<String>),
}
