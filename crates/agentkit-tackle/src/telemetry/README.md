# telemetry

Structured logging and OpenTelemetry signals, per the switchboard-otel
pattern.

## What lives here

```
mod.rs   the tracing subscriber setup: a fmt layer plus, when an OTLP
         endpoint is configured, the OTel trace layer; credentials are
         redacted at the type level and tool arguments in logs
```

All harness diagnostics go to **stderr**, never stdout — on the stdio
transport stdout is the ACP JSON-RPC wire and must stay clean. The OTLP
exporter initialises when `OTEL_EXPORTER_OTLP_ENDPOINT` is set (or
`OTEL_TRACES_EXPORTER` names an exporter); otherwise the fmt layer alone
runs. Initialisation failure falls back to fmt-only.

## Dependencies

- depends on: `opentelemetry`, `tracing-subscriber`
- depended on by: `main` (boots telemetry before the engine graph)
