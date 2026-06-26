use crate::config::{ModelConfig, ProviderConfig};
use crate::models::{EnrichedModel, ModelCapabilities, ModelPricing, ProviderModelInfo};
use std::{collections::HashMap, path::Path};

pub struct ModelDb {
    merged: HashMap<String, EnrichedModel>,
}

impl ModelDb {
    pub fn new(
        model_overrides: HashMap<String, ModelConfig>,
        providers: &HashMap<String, ProviderConfig>,
    ) -> Self {
        Self::from_snapshot(
            model_overrides,
            providers,
            agentkit_models::bundled_snapshot_parsed(),
        )
    }

    pub fn from_snapshot_path(
        path: &Path,
        model_overrides: HashMap<String, ModelConfig>,
        providers: &HashMap<String, ProviderConfig>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let bytes = std::fs::read(path)?;
        let snapshot = serde_json::from_slice(&bytes)?;
        Ok(Self::from_snapshot(model_overrides, providers, snapshot))
    }

    pub fn from_snapshot(
        model_overrides: HashMap<String, ModelConfig>,
        providers: &HashMap<String, ProviderConfig>,
        snapshot: serde_json::Value,
    ) -> Self {
        let bundled_models = snapshot
            .get("models")
            .and_then(|m| m.as_object())
            .cloned()
            .unwrap_or_default();
        let bundled_providers = snapshot
            .get("providers")
            .and_then(|p| p.as_object())
            .cloned()
            .unwrap_or_default();

        let mut merged: HashMap<String, EnrichedModel> = HashMap::new();
        let mut seen = std::collections::HashSet::new();

        for (model_id, bundled_entry) in &bundled_models {
            let ctx = bundled_entry
                .get("context_window")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32);
            let mx = bundled_entry
                .get("max_output")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32);
            let caps = bundled_entry
                .get("capabilities")
                .map(|c| ModelCapabilities {
                    tool_calling: c.get("tool_calling").and_then(|v| v.as_bool()),
                    reasoning: c.get("reasoning").and_then(|v| v.as_bool()),
                    structured_output: c.get("structured_output").and_then(|v| v.as_bool()),
                });

            let mut providers_info = Vec::new();
            for (prov_id, prov_data) in &bundled_providers {
                if let Some(pmodels) = prov_data.get("models").and_then(|m| m.as_object()) {
                    if pmodels.contains_key(model_id) {
                        let pcfg = pmodels.get(model_id).unwrap();
                        providers_info.push(ProviderModelInfo {
                            identity: prov_id.clone(),
                            billing: prov_data
                                .get("billing")
                                .and_then(|v| v.as_str())
                                .unwrap_or("pay_as_you_go")
                                .to_string(),
                            pricing: Some(ModelPricing {
                                input_per_mtok: pcfg
                                    .get("input_per_mtok")
                                    .and_then(|v| v.as_f64())
                                    .unwrap_or(0.0),
                                output_per_mtok: pcfg
                                    .get("output_per_mtok")
                                    .and_then(|v| v.as_f64())
                                    .unwrap_or(0.0),
                                cache_read_per_mtok: pcfg
                                    .get("cache_read_per_mtok")
                                    .and_then(|v| v.as_f64()),
                                cache_write_per_mtok: pcfg
                                    .get("cache_write_per_mtok")
                                    .and_then(|v| v.as_f64()),
                                reasoning_per_mtok: pcfg
                                    .get("reasoning_per_mtok")
                                    .and_then(|v| v.as_f64()),
                            }),
                        });
                    }
                }
            }

            let override_entry = model_overrides.get(model_id);
            let final_ctx = override_entry.and_then(|o| o.context_window).or(ctx);
            let final_mx = override_entry.and_then(|o| o.max_output).or(mx);
            let final_caps = override_entry
                .and_then(|o| o.capabilities.as_ref())
                .map(|c| ModelCapabilities {
                    tool_calling: c
                        .tool_calling
                        .or(caps.as_ref().and_then(|c| c.tool_calling)),
                    reasoning: c.reasoning.or(caps.as_ref().and_then(|c| c.reasoning)),
                    structured_output: c
                        .structured_output
                        .or(caps.as_ref().and_then(|c| c.structured_output)),
                })
                .or(caps);

            merged.insert(
                model_id.clone(),
                EnrichedModel {
                    id: model_id.clone(),
                    context_window: final_ctx,
                    max_output: final_mx,
                    capabilities: final_caps,
                    providers: providers_info,
                },
            );
            seen.insert(model_id.clone());
        }

        for (model_id, override_cfg) in &model_overrides {
            let providers_info = Vec::new();
            let entry = EnrichedModel {
                id: model_id.clone(),
                context_window: override_cfg.context_window,
                max_output: override_cfg.max_output,
                capabilities: override_cfg
                    .capabilities
                    .as_ref()
                    .map(|c| ModelCapabilities {
                        tool_calling: c.tool_calling,
                        reasoning: c.reasoning,
                        structured_output: c.structured_output,
                    }),
                providers: providers_info,
            };
            if !seen.contains(model_id) {
                merged.insert(model_id.clone(), entry);
            }
            seen.insert(model_id.clone());
        }

        for (prov_identity, prov_cfg) in providers {
            if let Some(ref model_list) = prov_cfg.models {
                for model_id in model_list {
                    if !seen.contains(model_id) {
                        merged.entry(model_id.clone()).or_insert(EnrichedModel {
                            id: model_id.clone(),
                            context_window: None,
                            max_output: None,
                            capabilities: None,
                            providers: Vec::new(),
                        });
                    }
                    let entry = merged.get_mut(model_id).unwrap();
                    let pricing = &prov_cfg.pricing;
                    let per_model = pricing.models.get(model_id);
                    entry.providers.push(ProviderModelInfo {
                        identity: prov_identity.clone(),
                        billing: prov_cfg.billing.to_string(),
                        pricing: Some(ModelPricing {
                            input_per_mtok: per_model
                                .and_then(|m| m.input_per_mtok)
                                .unwrap_or(pricing.input_per_mtok),
                            output_per_mtok: per_model
                                .and_then(|m| m.output_per_mtok)
                                .unwrap_or(pricing.output_per_mtok),
                            cache_read_per_mtok: per_model
                                .and_then(|m| m.cache_read_per_mtok)
                                .or(pricing.cache_read_per_mtok),
                            cache_write_per_mtok: per_model
                                .and_then(|m| m.cache_write_per_mtok)
                                .or(pricing.cache_write_per_mtok),
                            reasoning_per_mtok: per_model
                                .and_then(|m| m.reasoning_per_mtok)
                                .or(pricing.reasoning_per_mtok),
                        }),
                    });
                }
            }
        }

        Self { merged }
    }

    pub fn lookup(&self, model_id: &str) -> Option<&EnrichedModel> {
        self.merged.get(model_id)
    }

    pub fn all(&self) -> impl Iterator<Item = &EnrichedModel> {
        self.merged.values()
    }
}
