use opentelemetry::trace::Status;
use tracing_opentelemetry::OpenTelemetrySpanExt;

pub fn emit_traces() {
    let root = tracing::info_span!("process_batch");
    let _root_guard = root.enter();

    {
        let child = tracing::info_span!("fetch_users", db.table = "users", db.system = "postgres");
        let _child_guard = child.enter();

        tracing::info!(db.cache_miss = true, "cache miss");
        tracing::warn!(user.id = 42, "user 42 not found in cache");
    }

    {
        let child = tracing::info_span!("send_notifications", notification.type = "email");
        let _child_guard = child.enter();

        tracing::error!(error.message = "timeout", "notification delivery failed");
    }

    tracing::Span::current().set_status(Status::Ok);
}
