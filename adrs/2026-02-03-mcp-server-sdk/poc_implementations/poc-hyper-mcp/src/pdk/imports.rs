#![allow(unused)]
use super::types::*;
use extism_pdk::{Error, Json, host_fn};
use std::result::Result;

/// create_elicitation Request user input through the client's elicitation interface.
///
/// Plugins can use this to ask users for input, decisions, or confirmations. This is useful for interactive plugins that need user guidance during tool execution. Returns the user's response with action and optional form data.
/// It takes input of CreateElicitationRequestParamWithTimeout ()
/// And it returns an output CreateElicitationResult ()
pub(crate) fn create_elicitation(
    input: ElicitRequestParamWithTimeout,
) -> Result<ElicitResult, Error> {
    let Json(res) = unsafe { raw_imports::create_elicitation(Json(input))? };

    Ok(res)
}

/// create_message Request message creation through the client's sampling interface.
///
/// Plugins can use this to have the client create messages, typically with AI assistance. This is used when plugins need intelligent text generation or analysis. Returns the generated message with model information.
/// It takes input of CreateMessageRequestParam ()
/// And it returns an output CreateMessageResult ()
#[allow(unused)]
pub(crate) fn create_message(
    input: CreateMessageRequestParam,
) -> Result<CreateMessageResult, Error> {
    let Json(res) = unsafe { raw_imports::create_message(Json(input))? };

    Ok(res)
}

/// list_roots List the client's root directories or resources.
///
/// Plugins can query this to discover what root resources (typically file system roots) are available on the client side. This helps plugins understand the scope of resources they can access.
/// And it returns an output ListRootsResult ()
pub(crate) fn list_roots() -> Result<ListRootsResult, Error> {
    let Json(res) = unsafe { raw_imports::list_roots()? };

    Ok(res)
}

/// notify_logging_message Send a logging message to the client.
///
/// Plugins use this to report diagnostic, informational, warning, or error messages. The client's logging level determines which messages are processed.
/// It takes input of LoggingMessageNotificationParam ()
pub(crate) fn notify_logging_message(input: LoggingMessageNotificationParam) -> Result<(), Error> {
    unsafe { raw_imports::notify_logging_message(Json(input))? }
    Ok(())
}

/// notify_progress Send a progress notification to the client.
///
/// Plugins use this to report progress during long-running operations. This allows clients to display progress bars or status information to users.
/// It takes input of ProgressNotificationParam ()
pub(crate) fn notify_progress(input: ProgressNotificationParam) -> Result<(), Error> {
    unsafe { raw_imports::notify_progress(Json(input))? }
    Ok(())
}

/// notify_prompt_list_changed Notify the client that the list of available prompts has changed.
///
/// Plugins should call this when they add, remove, or modify their available prompts. The client will typically refresh its prompt list in response.
pub(crate) fn notify_prompt_list_changed() -> Result<(), Error> {
    unsafe { raw_imports::notify_prompt_list_changed()? }
    Ok(())
}

/// notify_resource_list_changed Notify the client that the list of available resources has changed.
///
/// Plugins should call this when they add, remove, or modify their available resources. The client will typically refresh its resource list in response.
pub(crate) fn notify_resource_list_changed() -> Result<(), Error> {
    unsafe { raw_imports::notify_resource_list_changed()? }
    Ok(())
}

/// notify_resource_updated Notify the client that a specific resource has been updated.
///
/// Plugins should call this when they modify the contents of a resource. The client can use this to invalidate caches and refresh resource displays.
/// It takes input of types::ResourceUpdatedNotificationParam ()
pub(crate) fn notify_resource_updated(
    input: ResourceUpdatedNotificationParam,
) -> Result<(), Error> {
    unsafe { raw_imports::notify_resource_updated(Json(input))? }
    Ok(())
}

/// notify_tool_list_changed Notify the client that the list of available tools has changed.
///
/// Plugins should call this when they add, remove, or modify their available tools. The client will typically refresh its tool list in response.
pub(crate) fn notify_tool_list_changed() -> Result<(), Error> {
    unsafe { raw_imports::notify_tool_list_changed()? }
    Ok(())
}

mod raw_imports {
    use super::*;
    #[host_fn]
    extern "ExtismHost" {
        pub(crate) fn create_elicitation(
            input: Json<ElicitRequestParamWithTimeout>,
        ) -> Json<ElicitResult>;

        pub(crate) fn create_message(
            input: Json<CreateMessageRequestParam>,
        ) -> Json<CreateMessageResult>;

        pub(crate) fn list_roots() -> Json<ListRootsResult>;

        pub(crate) fn notify_logging_message(input: Json<LoggingMessageNotificationParam>);

        pub(crate) fn notify_progress(input: Json<ProgressNotificationParam>);

        pub(crate) fn notify_prompt_list_changed();

        pub(crate) fn notify_resource_list_changed();

        pub(crate) fn notify_resource_updated(input: Json<ResourceUpdatedNotificationParam>);

        pub(crate) fn notify_tool_list_changed();
    }
}
