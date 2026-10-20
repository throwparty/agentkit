//! The `/context` report: an approximate breakdown of what the next model
//! request would carry, rendered as the fixed plain-text layout clients
//! display verbatim.

use crate::providers::{ChatContent, ChatMessage, ChatRole, ToolDefinition};
use crate::store::TurnKind;

/// Everything the report counts. `prior` is the tagged prior-message
/// assembly (the current turn excluded); `window` is zero when the model
/// has no context-window metadata.
pub struct ReportInputs<'a> {
    pub model: &'a str,
    pub system: &'a str,
    pub tools: &'a [ToolDefinition],
    pub prior: &'a [(ChatMessage, TurnKind)],
    pub window: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Default, PartialEq)]
struct Counts {
    system: u64,
    tools: u64,
    user: u64,
    assistant: u64,
    summaries: u64,
    images: u64,
}

impl Counts {
    fn used(&self) -> u64 {
        self.system + self.tools + self.user + self.assistant + self.summaries + self.images
    }
}

/// Renders the report exactly as clients should display it: plain text,
/// ASCII only, one decimal place for shares, four for USD cost.
#[must_use]
pub fn render(inputs: &ReportInputs<'_>) -> String {
    let counts = count(inputs);
    let used = counts.used();
    let known_window = inputs.window > 0;
    let mut out = String::new();

    if known_window {
        out.push_str(&format!(
            "Context window: ~{} / {} tokens ({})\n",
            group(used),
            group(inputs.window),
            share(used, inputs.window)
        ));
    } else {
        out.push_str(&format!(
            "Context window: ~{} tokens (window unknown)\n",
            group(used)
        ));
    }
    out.push_str(&format!("Model: {}\n\n", inputs.model));
    out.push_str(&row("Component", 0, "Share", true));
    out.push_str(&row(
        "System prompt",
        counts.system,
        &share_for(counts.system, inputs.window),
        false,
    ));
    out.push_str(&row(
        "Tool definitions",
        counts.tools,
        &share_for(counts.tools, inputs.window),
        false,
    ));
    out.push_str(&row(
        "User messages",
        counts.user,
        &share_for(counts.user, inputs.window),
        false,
    ));
    out.push_str(&row(
        "Assistant messages",
        counts.assistant,
        &share_for(counts.assistant, inputs.window),
        false,
    ));
    out.push_str(&row(
        "Compaction summaries",
        counts.summaries,
        &share_for(counts.summaries, inputs.window),
        false,
    ));
    out.push_str(&row(
        "Images",
        counts.images,
        &share_for(counts.images, inputs.window),
        false,
    ));
    out.push_str(&format!("  {}\n", "-".repeat(38)));
    out.push_str(&row("Used", used, &share_for(used, inputs.window), false));
    if known_window {
        let free = inputs.window.saturating_sub(used);
        out.push_str(&row("Free", free, &share_for(free, inputs.window), false));
    }
    out.push('\n');
    out.push_str(&format!(
        "Session totals: {} input | {} output",
        group(inputs.input_tokens),
        group(inputs.output_tokens)
    ));
    if let Some(cost) = inputs.cost_usd {
        out.push_str(&format!(" | ${cost:.4}"));
    }
    out.push('\n');
    out.push_str("Estimates use 4 bytes per token; images are counted by payload size.\n");
    out
}

fn count(inputs: &ReportInputs<'_>) -> Counts {
    let mut counts = Counts {
        system: tokens(inputs.system.len() as u64),
        tools: tokens(
            serde_json::to_vec(inputs.tools)
                .map(|json| json.len() as u64)
                .unwrap_or(0),
        ),
        ..Counts::default()
    };
    for (message, kind) in inputs.prior {
        for part in &message.content {
            match part {
                ChatContent::Text(text) => {
                    let bytes = text.len() as u64;
                    if *kind == TurnKind::Compaction {
                        counts.summaries += tokens(bytes);
                    } else if message.role == ChatRole::User {
                        counts.user += tokens(bytes);
                    } else {
                        counts.assistant += tokens(bytes);
                    }
                }
                ChatContent::Image { url } => {
                    counts.images += tokens(image_payload_len(url));
                }
            }
        }
    }
    counts
}

fn tokens(bytes: u64) -> u64 {
    crate::scripts::host::estimate_tokens(bytes)
}

/// A `data:` URL's payload (after the comma); a remote URI's own length.
fn image_payload_len(url: &str) -> u64 {
    url.split_once(',')
        .map(|(_, payload)| payload.len())
        .unwrap_or(url.len()) as u64
}

fn share_for(tokens: u64, window: u64) -> String {
    if window == 0 {
        "-".to_owned()
    } else {
        share(tokens, window)
    }
}

fn share(tokens: u64, window: u64) -> String {
    format!("{:.1}%", tokens as f64 * 100.0 / window as f64)
}

fn row(label: &str, tokens: u64, share: &str, header: bool) -> String {
    if header {
        format!("  {label:<20}  {:>9}   {share:>5}\n", "Tokens")
    } else {
        format!("  {label:<20}  {:>9}   {share:>5}\n", group(tokens))
    }
}

fn group(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{render, ReportInputs};
    use crate::providers::{ChatContent, ChatMessage, ChatRole};
    use crate::store::TurnKind;

    fn sample<'a>(
        system: &'a str,
        prior: &'a [(ChatMessage, TurnKind)],
        window: u64,
        cost_usd: Option<f64>,
    ) -> ReportInputs<'a> {
        ReportInputs {
            model: "test/model",
            system,
            tools: &[],
            prior,
            window,
            input_tokens: 48_210,
            output_tokens: 12_340,
            cost_usd,
        }
    }

    #[test]
    fn known_window_and_cost_render_the_exact_layout() {
        let system = "s".repeat(400);
        let user = ChatMessage::user("u".repeat(40));
        let assistant = ChatMessage::assistant("a".repeat(80));
        let summary = ChatMessage::user("c".repeat(20));
        let image = ChatMessage::with_content(
            ChatRole::User,
            vec![ChatContent::Image {
                url: format!("data:image/png;base64,{}", "A".repeat(40)),
            }],
        );
        let prior = vec![
            (user, TurnKind::Interaction),
            (assistant, TurnKind::Interaction),
            (summary, TurnKind::Compaction),
            (image, TurnKind::Interaction),
        ];
        let rendered = render(&sample(&system, &prior, 1_000, Some(0.42)));
        assert_eq!(
            rendered,
            "Context window: ~145 / 1,000 tokens (14.5%)\n\
             Model: test/model\n\
             \n\
             \x20 Component                Tokens   Share\n\
             \x20 System prompt               100   10.0%\n\
             \x20 Tool definitions              0    0.0%\n\
             \x20 User messages                10    1.0%\n\
             \x20 Assistant messages           20    2.0%\n\
             \x20 Compaction summaries          5    0.5%\n\
             \x20 Images                       10    1.0%\n\
             \x20 --------------------------------------\n\
             \x20 Used                        145   14.5%\n\
             \x20 Free                        855   85.5%\n\
             \n\
             Session totals: 48,210 input | 12,340 output | $0.4200\n\
             Estimates use 4 bytes per token; images are counted by payload size.\n"
        );
    }

    #[test]
    fn unknown_window_omits_shares_and_free() {
        let system = "s".repeat(40);
        let rendered = render(&sample(&system, &[], 0, None));
        assert_eq!(
            rendered,
            "Context window: ~10 tokens (window unknown)\n\
             Model: test/model\n\
             \n\
             \x20 Component                Tokens   Share\n\
             \x20 System prompt                10       -\n\
             \x20 Tool definitions              0       -\n\
             \x20 User messages                 0       -\n\
             \x20 Assistant messages            0       -\n\
             \x20 Compaction summaries          0       -\n\
             \x20 Images                        0       -\n\
             \x20 --------------------------------------\n\
             \x20 Used                         10       -\n\
             \n\
             Session totals: 48,210 input | 12,340 output\n\
             Estimates use 4 bytes per token; images are counted by payload size.\n"
        );
    }

    #[test]
    fn over_window_clamps_free_and_reports_the_real_percentage() {
        let system = "s".repeat(4_400);
        let rendered = render(&sample(&system, &[], 1_000, None));
        assert!(rendered.contains("~1,100 / 1,000 tokens (110.0%)"));
        let free = rendered
            .lines()
            .find(|line| line.trim_start().starts_with("Free"))
            .expect("free row");
        assert_eq!(
            free.split_whitespace().collect::<Vec<_>>(),
            ["Free", "0", "0.0%"]
        );
    }
}
