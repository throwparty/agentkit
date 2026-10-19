//! Mapping the domain session-config options and notices onto the ACP
//! wire types.

use crate::agent::config_options::{
    ConfigOption, ConfigOptionCategory, ConfigOptionKind, Notice, NoticeSeverity,
};
use agent_client_protocol::schema::v1::{
    Notice as WireNotice, NoticeSeverity as WireSeverity, SessionConfigId, SessionConfigKind,
    SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelect,
    SessionConfigSelectOption, SessionConfigValueId,
};

/// Maps a domain notice onto the wire type.
pub(crate) fn wire_notice(notice: &Notice) -> WireNotice {
    let mut wire = WireNotice::new(
        match notice.severity {
            NoticeSeverity::Info => WireSeverity::Info,
            NoticeSeverity::Warning => WireSeverity::Warning,
        },
        notice.message.clone(),
    );
    if let Some(description) = &notice.description {
        wire = wire.description(Some(description.clone()));
    }
    wire
}

/// Maps a domain config option onto the wire type.
pub(crate) fn wire_option(option: &ConfigOption) -> SessionConfigOption {
    let ConfigOptionKind::Select { current, options } = &option.kind;
    let select_options: Vec<SessionConfigSelectOption> = options
        .iter()
        .map(|option| {
            SessionConfigSelectOption::new(
                SessionConfigValueId::new(option.value.clone()),
                option.label.clone(),
            )
        })
        .collect();
    let select =
        SessionConfigSelect::new(SessionConfigValueId::new(current.clone()), select_options);
    SessionConfigOption::new(
        SessionConfigId::new(option.id.clone()),
        option.name.clone(),
        SessionConfigKind::Select(select),
    )
    .category(match &option.category {
        ConfigOptionCategory::Model => SessionConfigOptionCategory::Model,
        ConfigOptionCategory::Other(name) => SessionConfigOptionCategory::Other(name.clone()),
    })
}

/// Maps a slice of domain config options onto the wire type.
pub(crate) fn wire_options(options: &[ConfigOption]) -> Vec<SessionConfigOption> {
    options.iter().map(wire_option).collect()
}
