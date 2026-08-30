# Switchboard: API Surfaces

**Status:** draft  **Created:** 2026-08-31  **Author:** adrian

The switchboard (adrs/2026-06-14-switchboard) conflates three orthogonal concepts: wire format (API surface), billing model, and provider identity. ApiSurface::Openai selects /responses vs /chat/completions by billing model, and ProviderConfig forces one surface per provider. This makes it impossible to represent a gateway provider like OpenCode Zen (https://opencode.ai/docs/zen/), which serves models across three wire formats under one base URL and API key.

Zen's four endpoints map to wire formats:
1. /zen/v1/chat/completions - OpenAI Chat Completions
2. /zen/v1/responses - OpenAI Responses
3. /zen/v1/messages - Anthropic Messages
4. /zen/v1/models/gemini-* - Gemini (no client; deferred)

The switchboard already ships clients for the first three formats. The translation layer (ConversationHandler) is lossy and unnecessary: each format has native semantics (instructions, cache_control, reasoning effort) that cannot be faithfully mapped to Chat Completions. The switchboard should proxy native formats within a surface, not translate between surfaces.


## Problem

The switchboard cannot represent a provider that serves models across multiple wire formats, because wire format is tangled with billing model and provider identity. OpenCode Zen is such a provider: one base URL and API key serving three wire formats. Supporting it requires untangling these concepts and dropping the lossy translation layer in favour of native proxying.

## Goals

- Make wire format a first-class concept decoupled from billing and provider identity
- Rename surfaces to explicit wire formats (openai-chat-completions, openai-responses, anthropic-messages)
- Drop translation and proxy native formats within a surface
- Allow a provider to serve multiple wire formats
- Add OpenCode Zen as the first multi-surface provider

## Non-goals

- Gemini API surface (no client; deferred)
- Cross-surface translation (dropped by design)
- Zen team/workspace features (roles, model access control, bring-your-own-key)


## Functional Requirements

### FR-001: Wire Format Decoupling

Wire format (API surface) is a first-class concept, decoupled from billing model and provider identity

**Slug:** `wire-format-decoupling`

### FR-002: Surface Renaming

Surfaces are renamed to explicit wire formats: openai-chat-completions, openai-responses, anthropic-messages

**Slug:** `surface-renaming`

### FR-003: Proxy-Native Routing

Requests are proxied in their native wire format; the translation layer (ConversationHandler) is removed

**Slug:** `proxy-native-routing`

### FR-004: Multi-Surface Providers

A provider can serve models across multiple wire formats

**Slug:** `multi-surface-providers`

### FR-005: Zen Provider

OpenCode Zen is configurable as a provider with base_url https://opencode.ai/zen/v1, bearer_token auth, and pay_as_you_go billing

**Slug:** `zen-provider`

### FR-006: Zen Model Metadata

Zen model facts and pricing are sourced from the models.dev opencode provider snapshot

**Slug:** `zen-model-metadata`

### FR-007: Zen Auth

Zen's static API key is stored and presented via the existing bearer_token credential path

**Slug:** `zen-auth`

## Non-functional Requirements

### NFR-001: No Translation

No cross-surface translation is implemented; the switchboard routes within a surface only

**Slug:** `no-translation`

### NFR-002: Reuse Existing Clients

Existing wire-format clients (Chat Completions, Responses, Messages) are reused; no new wire-format client is implemented

**Slug:** `reuse-existing-clients`

## Acceptance Criteria

### AC-001: Surface Enum

The ApiSurface enum has three explicit variants: openai-chat-completions, openai-responses, anthropic-messages

**Slug:** `surface-enum`

### AC-002: Billing Decoupled

Endpoint selection is driven by surface, not billing model

**Slug:** `billing-decoupled`

### AC-003: Multi-Surface Config

A provider config can declare multiple surfaces

**Slug:** `multi-surface-config`

### AC-004: Zen Routing

A Zen request routes to the correct endpoint per surface (chat/completions, responses, messages)

**Slug:** `zen-routing`

### AC-005: Native Proxy

Request and response bodies pass through unchanged (no translation)

**Slug:** `native-proxy`

## Edge Cases

### EC-001: Auth Presentation

Zen's single key must be presented correctly per surface (Bearer for OpenAI surfaces, x-api-key for the Anthropic surface); verify Zen's /messages endpoint accepts the expected header

**Slug:** `auth-presentation`
### EC-002: Model Deprecation

Zen deprecates models on a schedule; deprecated models must not be advertised as routable

**Slug:** `model-deprecation`
### EC-003: Cross-Provider Surface Variance

The same model may be served via different surfaces by different providers; surface is a property of the (provider, model) pair

**Slug:** `cross-provider-surface-variance`

