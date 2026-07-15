use opentelemetry::logs::{AnyValue, LogRecord, Logger, Severity};
use opentelemetry::Key;
use opentelemetry_sdk::logs::SdkLogger;
use std::collections::HashMap;
use tracing::field::Visit;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;

pub struct OtelLogLayer {
    logger: SdkLogger,
}

impl OtelLogLayer {
    pub fn new(logger: SdkLogger) -> Self {
        OtelLogLayer { logger }
    }
}

impl<S> Layer<S> for OtelLogLayer
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut fields = HashMap::new();
        event.record(&mut FieldVisitor(&mut fields));

        let message = fields
            .remove("message")
            .or_else(|| fields.remove("log.message"))
            .map(|v| render_any_value(&v))
            .unwrap_or_default();

        let severity = match *event.metadata().level() {
            tracing::Level::ERROR => Severity::Error,
            tracing::Level::WARN => Severity::Warn,
            tracing::Level::INFO => Severity::Info,
            tracing::Level::DEBUG => Severity::Debug,
            tracing::Level::TRACE => Severity::Trace,
        };

        let mut record = self.logger.create_log_record();
        record.set_observed_timestamp(std::time::SystemTime::now());
        record.set_severity_number(severity);
        record.set_body(message.into());

        for (key, value) in &fields {
            record.add_attribute(key.clone(), value.clone());
        }

        self.logger.emit(record);
    }
}

fn render_any_value(v: &AnyValue) -> String {
    match v {
        AnyValue::String(s) => s.to_string(),
        AnyValue::Boolean(b) => b.to_string(),
        AnyValue::Int(i) => i.to_string(),
        AnyValue::Double(f) => f.to_string(),
        AnyValue::ListAny(items) => items
            .iter()
            .map(render_any_value)
            .collect::<Vec<_>>()
            .join(", "),
        _ => format!("{v:?}"),
    }
}

struct FieldVisitor<'a>(&'a mut HashMap<Key, AnyValue>);

impl<'a> Visit for FieldVisitor<'a> {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.insert(
            Key::new(field.name()),
            AnyValue::String(value.to_string().into()),
        );
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.0
            .insert(Key::new(field.name()), AnyValue::Boolean(value));
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.0
            .insert(Key::new(field.name()), AnyValue::Int(value));
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.0
            .insert(Key::new(field.name()), AnyValue::Int(value as i64));
    }

    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        self.0
            .insert(Key::new(field.name()), AnyValue::Double(value));
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let s = format!("{value:?}");
        self.0
            .insert(Key::new(field.name()), AnyValue::String(s.into()));
    }
}
