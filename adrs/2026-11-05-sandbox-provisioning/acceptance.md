# Acceptance: Sandbox Provisioning

Recorded 2026-11-05. Commands run from the repository root through the project
devShell.

## Unit and integration tests

```bash
nix develop ./nix --command cargo test -p agentkit-litterbox --lib
nix develop ./nix --command cargo test -p agentkit-litterbox --doc
```

332 passed, 0 failed, 8 ignored in the library target. The ignored tests are the
Docker suite. The doc target runs the compile_fail facet doctest: 1 passed.
The doctor integration test target has 2 passed, including a misconfigured run.

## Docker suite

```bash
LITTERBOX_DOCKER_TESTS=1 nix develop ./nix --command cargo test -p agentkit-litterbox --lib -- --ignored
```

8 passed, 0 failed: connection and image, lifecycle, limits, conformance,
idempotent create/inspect/delete, ordered command steps, the failed command
step, and the failed-create rollback.

## Lints and boundaries

```bash
nix develop ./nix --command cargo clippy -p agentkit-litterbox --all-targets -- -D warnings
nix develop ./nix --command arch-lint check
nix develop ./nix --command bash hack/arch-lint-negative.sh
```

Clippy is clean. `arch-lint check` reports 0 errors across 353 files. The
negative fixture rejects the planted contract→backend, contract→provision,
CLI→backend, CLI→SCM, CLI→provision, MCP→provision, MCP error→backend,
config→backend, config→provision, registry→orchestrator, registry→provision,
orchestrator→backend, backend→provision and provision→orchestrator imports.

## Criteria

| Criterion | Evidence |
|---|---|
| AC-001 ordered sequence | `provision::resolve::tests::a_sequence_resolves_in_order`, Docker `command_steps_run_in_order_in_the_workspace` (the later step reads the earlier step's file) |
| AC-002 per-backend selection | `provision::resolve::tests::each_backend_resolves_only_its_own_sequence` |
| AC-003 unlisted backend refused | `provision::resolve::tests::an_unlisted_backend_is_refused`, `orchestrator::service::seam_tests::a_resolver_failure_leaves_no_branch` |
| AC-004 unknown identity | `provision::resolve::tests::an_unknown_identity_is_refused` |
| AC-005 unsupported kind | `provision::resolve::tests::an_unsupported_kind_is_refused` |
| AC-006 setup-command replaced | `config_loader::tests::setup_command_is_rejected_with_the_replacement`, `the_committed_project_configurations_still_parse`, Docker `command_steps_run_in_order_in_the_workspace` (relative workspace path and forwarded-port environment; confinement is the existing create path) |
| AC-007 consent unchanged | `orchestrator::service::seam_tests::a_listed_project_may_run_its_setup_command`, `accepting_every_project_permits_its_setup_command`, `a_project_setup_command_needs_permission`; the permit tests assert the recorded request carries the step |
| AC-008 atomic failure | `orchestrator::service::seam_tests::a_failed_create_removes_the_branch_it_made`, Docker `a_failed_command_step_names_the_step_and_rolls_back` |
| AC-009 provenance per step | `sandbox::environment::tests::a_step_reports_its_identity_kind_and_variable_names_without_values`, Docker `command_steps_run_in_order_in_the_workspace` (inspects the record create returned), and the label round-trip in `enumeration_rebuilds_the_record_from_labels` |
| AC-010 doctor does not mutate | `cli::tests::the_doctor_builder_marks_a_command_step_denied_without_mutating`, `a_denied_step_is_reported_in_json_with_its_notices`, `a_resolver_error_sets_the_class_and_the_exit_code`, and `tests/doctor.rs` (`a_provisioned_doctor_run_creates_no_branch_or_record`, `a_misconfigured_doctor_run_creates_no_branch`) which drive the entry point and assert the branch list, checkout contents and metadata store are unchanged. |
| AC-011 matrix does not leak | `orchestrator::service::seam_tests::a_request_carries_only_the_selected_backends_steps` |
| AC-012 local override | `config::tests::the_local_supported_set_replaces_the_project_one`, `the_local_definitions_replace_the_project_definitions`, `an_empty_local_definition_list_removes_inherited_sequences` (a deliberate empty list loads and clears the project's sequences), `an_empty_local_sequence_removes_the_project_sequence`, `config_loader::tests::a_local_backend_sequence_reaches_the_merged_sequences` (a local sequence is project-owned while machine backend settings still come from the user configuration) |
| AC-013 empty sequence | `provision::resolve::tests::a_backend_with_no_sequence_runs_no_steps`, `no_definitions_and_no_supported_set_is_empty` |
| AC-014 boundaries | `arch-lint check` and `hack/arch-lint-negative.sh`, including the CLI→provision, MCP→provision, contract→provision, config→provision and registry→provision plants |
| AC-015 unknown kind | `config::tests::an_unknown_provisioning_kind_is_rejected` |

## Note on stored labels

`Provenance.steps` is `#[serde(default)]`, so a sandbox created before this
change still enumerates: its label carries no `steps` field and deserialises as
an empty list. `sandbox::environment::tests::a_legacy_provenance_without_steps_deserialises`
covers that payload.

Docker labels are written when the container is created, before any step runs.
They therefore carry the base environment only; the record create returns, and
the record the orchestrator stores, carry the steps that actually completed. A
label never claims a step that did not run.
