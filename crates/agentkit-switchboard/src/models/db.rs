use crate::config::{ModelConfig, ProviderConfig};
use crate::models::{EnrichedModel, ModelCapabilities, ModelPricing, ProviderModelInfo};
use agentkit_models::ModelSnapshot;
use sqlx::SqlitePool;
use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
};

pub struct ModelDb {
    models: Vec<EnrichedModel>,
    provider_models: HashMap<String, Vec<String>>,
}

struct Instance {
    model: String,
    context_window: Option<u32>,
    max_output: Option<u32>,
    capabilities: Option<ModelCapabilities>,
    provider: ProviderModelInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ShapeFingerprint {
    context_window: u32,
    max_output: u32,
    tool_calling: bool,
    reasoning: bool,
    structured_output: bool,
}

impl Instance {
    fn shape(&self) -> Option<ShapeFingerprint> {
        let capabilities = self.capabilities.as_ref()?;
        Some(ShapeFingerprint {
            context_window: self.context_window?,
            max_output: self.max_output?,
            tool_calling: capabilities.tool_calling?,
            reasoning: capabilities.reasoning?,
            structured_output: capabilities.structured_output?,
        })
    }
}

impl ModelDb {
    pub fn new(providers: &HashMap<String, ProviderConfig>) -> Self {
        Self::from_snapshot(providers, agentkit_models::bundled_snapshot_parsed())
    }

    pub fn from_snapshot_path(
        path: &Path,
        providers: &HashMap<String, ProviderConfig>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let bytes = std::fs::read(path)?;
        let snapshot: ModelSnapshot = serde_json::from_slice(&bytes)?;
        Ok(Self::from_snapshot(providers, snapshot))
    }

    pub fn from_snapshot(
        providers: &HashMap<String, ProviderConfig>,
        snapshot: ModelSnapshot,
    ) -> Self {
        let mut by_model: BTreeMap<String, Vec<Instance>> = BTreeMap::new();
        let mut provider_models = HashMap::new();

        for (prov_identity, prov_cfg) in providers {
            let model_ids = served_models(prov_identity, prov_cfg, &snapshot);
            provider_models.insert(prov_identity.clone(), model_ids.clone());

            for model_id in model_ids {
                let snapshot_entry = snapshot.models.get(&model_id);
                let base_caps = snapshot_entry
                    .and_then(|entry| entry.capabilities.as_ref())
                    .map(|c| ModelCapabilities {
                        tool_calling: c.tool_calling,
                        reasoning: c.reasoning,
                        structured_output: c.structured_output,
                    });
                let overrides = prov_cfg.model_overrides.get(&model_id);

                by_model
                    .entry(model_id.clone())
                    .or_default()
                    .push(Instance {
                        model: model_id.clone(),
                        context_window: overrides
                            .and_then(|o| o.context_window)
                            .or_else(|| snapshot_entry.and_then(|e| e.context_window)),
                        max_output: overrides
                            .and_then(|o| o.max_output)
                            .or_else(|| snapshot_entry.and_then(|e| e.max_output)),
                        capabilities: merge_capabilities(base_caps, overrides),
                        provider: provider_info(prov_identity, prov_cfg, &model_id),
                    });
            }
        }

        let mut models = Vec::new();
        for (model_id, instances) in by_model {
            let mut complete: BTreeMap<ShapeFingerprint, Vec<Instance>> = BTreeMap::new();
            let mut unknown = Vec::new();
            for instance in instances {
                match instance.shape() {
                    Some(shape) => complete.entry(shape).or_default().push(instance),
                    None => unknown.push(vec![instance]),
                }
            }

            let group_count = complete.len() + unknown.len();
            if group_count == 1 {
                let group = complete
                    .values()
                    .next()
                    .or_else(|| unknown.first())
                    .expect("one shape group exists");
                models.push(alias(&model_id, group));
            }

            for group in complete.values() {
                models.extend(group.iter().map(qualified));
            }
            for group in unknown {
                models.extend(group.iter().map(qualified));
            }
        }

        models.sort_by(|a, b| {
            a.model
                .cmp(&b.model)
                .then_with(|| b.alias.cmp(&a.alias))
                .then_with(|| a.id.cmp(&b.id))
        });

        Self {
            models,
            provider_models,
        }
    }

    pub fn provider_models(&self) -> &HashMap<String, Vec<String>> {
        &self.provider_models
    }

    pub async fn sync_to_db(&self, pool: &SqlitePool) -> Result<(), sqlx::Error> {
        let now = chrono::Utc::now().timestamp();
        for model in self.models.iter().filter(|model| !model.alias) {
            let provider = &model.providers[0];
            sqlx::query(
                "INSERT OR REPLACE INTO models (id, provider, context_window, max_output, tool_calling, reasoning, structured_output, synced_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&model.id)
            .bind(&provider.identity)
            .bind(model.context_window.map(|v| v as i64))
            .bind(model.max_output.map(|v| v as i64))
            .bind(model.capabilities.as_ref().and_then(|c| c.tool_calling).map(|v| v as i64))
            .bind(model.capabilities.as_ref().and_then(|c| c.reasoning).map(|v| v as i64))
            .bind(model.capabilities.as_ref().and_then(|c| c.structured_output).map(|v| v as i64))
            .bind(now)
            .execute(pool)
            .await?;
        }
        Ok(())
    }

    pub fn lookup(&self, id: &str) -> Option<&EnrichedModel> {
        self.models.iter().find(|model| model.id == id)
    }

    pub fn lookup_for(&self, provider: &str, model_id: &str) -> Option<&EnrichedModel> {
        let id = format!("{provider}/{model_id}");
        self.lookup(&id)
    }

    pub fn lookup_qualified(&self, id: &str) -> Option<&EnrichedModel> {
        self.models
            .iter()
            .find(|model| !model.alias && model.id == id)
    }

    pub fn has_alias(&self, model_id: &str) -> bool {
        self.models
            .iter()
            .any(|model| model.alias && model.model == model_id)
    }

    pub fn all(&self) -> impl Iterator<Item = &EnrichedModel> {
        self.models.iter()
    }
}

fn served_models(identity: &str, config: &ProviderConfig, snapshot: &ModelSnapshot) -> Vec<String> {
    let mut models = match &config.models {
        Some(models) => models.clone(),
        None => snapshot
            .providers
            .get(identity)
            .map(|provider| provider.models.keys().cloned().collect())
            .unwrap_or_default(),
    };
    models.extend(config.model_overrides.keys().cloned());
    models.sort();
    models.dedup();
    models
}

fn alias(model_id: &str, group: &[Instance]) -> EnrichedModel {
    let first = &group[0];
    let mut providers: Vec<_> = group
        .iter()
        .map(|instance| instance.provider.clone())
        .collect();
    providers.sort_by(|a, b| a.identity.cmp(&b.identity));
    EnrichedModel {
        id: model_id.to_owned(),
        model: model_id.to_owned(),
        context_window: first.context_window,
        max_output: first.max_output,
        capabilities: first.capabilities.clone(),
        providers,
        alias: true,
    }
}

fn qualified(instance: &Instance) -> EnrichedModel {
    EnrichedModel {
        id: format!("{}/{}", instance.provider.identity, instance.model),
        model: instance.model.clone(),
        context_window: instance.context_window,
        max_output: instance.max_output,
        capabilities: instance.capabilities.clone(),
        providers: vec![instance.provider.clone()],
        alias: false,
    }
}

fn merge_capabilities(
    base: Option<ModelCapabilities>,
    overrides: Option<&ModelConfig>,
) -> Option<ModelCapabilities> {
    let overrides = overrides.and_then(|o| o.capabilities.as_ref());
    if base.is_none() && overrides.is_none() {
        return None;
    }
    let base = base.unwrap_or_default();
    Some(ModelCapabilities {
        tool_calling: overrides.and_then(|c| c.tool_calling).or(base.tool_calling),
        reasoning: overrides.and_then(|c| c.reasoning).or(base.reasoning),
        structured_output: overrides
            .and_then(|c| c.structured_output)
            .or(base.structured_output),
    })
}

fn provider_info(identity: &str, config: &ProviderConfig, model_id: &str) -> ProviderModelInfo {
    let pricing = &config.pricing;
    let per_model = pricing.models.get(model_id);
    ProviderModelInfo {
        identity: identity.to_string(),
        billing: config.billing.to_string(),
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
    }
}
