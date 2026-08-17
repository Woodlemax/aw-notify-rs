use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use aw_client_rust::blocking::AwClient;
use aw_client_rust::classes::{default_classes, ClassSetting};
use aw_models::{Bucket, Event};
use aw_pomodoro_service::ActivityObservation;
use chrono::{DateTime, Utc};
use fancy_regex::Regex;
use serde_json::{Map, Value};

const BUCKET_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const CLASS_REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const WATCHER_GRACE_SECONDS: i64 = 15;

pub struct ActivityWatchActivitySource {
    hostname: String,
    buckets: Option<BucketIds>,
    buckets_refreshed_at: Option<Instant>,
    classifier: Option<ActivityClassifier>,
    classes_refreshed_at: Option<Instant>,
}

#[derive(Clone)]
struct BucketIds {
    window: String,
    afk: String,
    browsers: Vec<String>,
}

struct ActivityClassifier {
    rules: Vec<CompiledRule>,
}

struct CompiledRule {
    category: Vec<String>,
    regex: Regex,
}

impl ActivityWatchActivitySource {
    pub fn new(hostname: impl Into<String>) -> Self {
        Self {
            hostname: hostname.into(),
            buckets: None,
            buckets_refreshed_at: None,
            classifier: None,
            classes_refreshed_at: None,
        }
    }

    pub fn observe(
        &mut self,
        client: &AwClient,
        now: Instant,
        wall_now: DateTime<Utc>,
    ) -> Result<ActivityObservation> {
        self.refresh_buckets_if_needed(client, now)?;
        let buckets = self
            .buckets
            .clone()
            .ok_or_else(|| anyhow!("ActivityWatch buckets are not initialized"))?;

        let afk_event = latest_current_event(client, &buckets.afk, wall_now)?;
        let afk_status = afk_event
            .data
            .get("status")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("latest AFK event has no string status"))?;
        if afk_status == "afk" {
            return Ok(ActivityObservation::afk());
        }
        if afk_status != "not-afk" {
            return Err(anyhow!("unknown AFK status: {afk_status}"));
        }

        let window_event = latest_current_event(client, &buckets.window, wall_now)?;
        self.refresh_classes_if_needed(client, now)?;
        let classifier = self
            .classifier
            .as_ref()
            .ok_or_else(|| anyhow!("Categorization rules are not initialized"))?;

        let mut combined_data = window_event.data.clone();
        if let Some(browser_event) =
            latest_active_browser_event(client, &buckets.browsers, &window_event, wall_now)?
        {
            for (key, value) in browser_event.data {
                combined_data.insert(format!("web_{key}"), value);
            }
        }

        Ok(ActivityObservation::active(
            classifier.classify(&combined_data)?,
        ))
    }

    fn refresh_buckets_if_needed(&mut self, client: &AwClient, now: Instant) -> Result<()> {
        let due = self.buckets.is_none()
            || self
                .buckets_refreshed_at
                .is_none_or(|last| now.saturating_duration_since(last) >= BUCKET_REFRESH_INTERVAL);
        if !due {
            return Ok(());
        }

        let buckets = client
            .get_buckets()
            .context("failed to read ActivityWatch buckets")?;
        self.buckets = Some(discover_buckets(&buckets, &self.hostname)?);
        self.buckets_refreshed_at = Some(now);
        Ok(())
    }

    fn refresh_classes_if_needed(&mut self, client: &AwClient, now: Instant) -> Result<()> {
        let due = self.classifier.is_none()
            || self
                .classes_refreshed_at
                .is_none_or(|last| now.saturating_duration_since(last) >= CLASS_REFRESH_INTERVAL);
        if !due {
            return Ok(());
        }

        let value = client
            .get_setting("classes")
            .context("failed to read ActivityWatch Categorization settings")?;
        let mut classes = if value.is_null() {
            Vec::new()
        } else {
            serde_json::from_value::<Vec<ClassSetting>>(value)
                .context("invalid ActivityWatch Categorization settings")?
        };
        if classes.is_empty() {
            classes = default_classes()
                .into_iter()
                .map(|(name, rule)| ClassSetting {
                    id: None,
                    name,
                    rule,
                    data: None,
                })
                .collect();
        }
        self.classifier = Some(ActivityClassifier::compile(&classes)?);
        self.classes_refreshed_at = Some(now);
        Ok(())
    }
}

impl ActivityClassifier {
    fn compile(classes: &[ClassSetting]) -> Result<Self> {
        let mut rules = Vec::new();
        for class in classes {
            if class.rule.spec_type != "regex" {
                continue;
            }
            let pattern = if class.rule.ignore_case {
                format!("(?i){}", class.rule.regex)
            } else {
                class.rule.regex.clone()
            };
            let regex = Regex::new(&pattern).with_context(|| {
                format!(
                    "invalid Categorization regex for {}",
                    class.name.join(" > ")
                )
            })?;
            rules.push(CompiledRule {
                category: class.name.clone(),
                regex,
            });
        }
        Ok(Self { rules })
    }

    fn classify(&self, data: &Map<String, Value>) -> Result<Option<Vec<String>>> {
        let mut category: Option<Vec<String>> = None;
        for rule in &self.rules {
            let matches =
                data.values()
                    .filter_map(Value::as_str)
                    .try_fold(false, |matched, value| {
                        if matched {
                            Ok(true)
                        } else {
                            rule.regex
                                .is_match(value)
                                .context("Categorization regex evaluation failed")
                        }
                    })?;
            if matches
                && category
                    .as_ref()
                    .is_none_or(|current| rule.category.len() >= current.len())
            {
                category = Some(rule.category.clone());
            }
        }
        Ok(category)
    }
}

fn discover_buckets(buckets: &HashMap<String, Bucket>, hostname: &str) -> Result<BucketIds> {
    let window = newest_bucket(buckets, hostname, "currentwindow")
        .ok_or_else(|| anyhow!("no currentwindow bucket found for host {hostname}"))?;
    let afk = newest_bucket(buckets, hostname, "afkstatus")
        .ok_or_else(|| anyhow!("no afkstatus bucket found for host {hostname}"))?;
    let browsers = buckets
        .values()
        .filter(|bucket| {
            bucket._type == "web.tab.current"
                && (bucket.hostname == hostname || bucket.hostname == "unknown")
        })
        .map(|bucket| bucket.id.clone())
        .collect();
    Ok(BucketIds {
        window: window.id.clone(),
        afk: afk.id.clone(),
        browsers,
    })
}

fn newest_bucket<'a>(
    buckets: &'a HashMap<String, Bucket>,
    hostname: &str,
    bucket_type: &str,
) -> Option<&'a Bucket> {
    buckets
        .values()
        .filter(|bucket| bucket.hostname == hostname && bucket._type == bucket_type)
        .max_by_key(|bucket| bucket.last_updated)
}

fn latest_current_event(
    client: &AwClient,
    bucket_id: &str,
    wall_now: DateTime<Utc>,
) -> Result<Event> {
    let event = client
        .get_events(bucket_id, None, None, Some(1))
        .with_context(|| format!("failed to read bucket {bucket_id}"))?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("bucket {bucket_id} has no events"))?;
    if !event_is_current(&event, wall_now) {
        return Err(anyhow!("latest event in {bucket_id} is stale"));
    }
    Ok(event)
}

fn latest_active_browser_event(
    client: &AwClient,
    bucket_ids: &[String],
    window_event: &Event,
    wall_now: DateTime<Utc>,
) -> Result<Option<Event>> {
    let app = window_event
        .data
        .get("app")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut latest: Option<Event> = None;
    for bucket_id in bucket_ids
        .iter()
        .filter(|bucket_id| browser_bucket_matches_app(bucket_id, app))
    {
        let Some(event) = client
            .get_events(bucket_id, None, None, Some(1))
            .with_context(|| format!("failed to read browser bucket {bucket_id}"))?
            .into_iter()
            .next()
        else {
            continue;
        };
        if !event_is_current(&event, wall_now) {
            continue;
        }
        if latest
            .as_ref()
            .is_none_or(|current| event.timestamp > current.timestamp)
        {
            latest = Some(event);
        }
    }
    Ok(latest)
}

fn event_is_current(event: &Event, wall_now: DateTime<Utc>) -> bool {
    let lag = wall_now.signed_duration_since(event.calculate_endtime());
    lag >= chrono::Duration::seconds(-5) && lag <= chrono::Duration::seconds(WATCHER_GRACE_SECONDS)
}

fn browser_bucket_matches_app(bucket_id: &str, app: &str) -> bool {
    let bucket = bucket_id.to_ascii_lowercase();
    let app = app.to_ascii_lowercase();
    [
        ("chrome", &["chrome", "chromium"][..]),
        ("firefox", &["firefox", "librewolf", "waterfox"][..]),
        ("edge", &["msedge", "microsoft edge"][..]),
        ("brave", &["brave"][..]),
        ("opera", &["opera"][..]),
        ("vivaldi", &["vivaldi"][..]),
    ]
    .iter()
    .any(|(browser, app_names)| {
        bucket.contains(browser) && app_names.iter().any(|name| app.contains(name))
    })
}

#[cfg(test)]
mod tests {
    use super::{browser_bucket_matches_app, ActivityClassifier};
    use aw_client_rust::classes::{CategorySpec, ClassSetting};
    use serde_json::{json, Map};

    fn class(name: &[&str], regex: &str, ignore_case: bool) -> ClassSetting {
        ClassSetting {
            id: None,
            name: name.iter().map(|part| (*part).to_string()).collect(),
            rule: CategorySpec {
                spec_type: "regex".to_string(),
                regex: regex.to_string(),
                ignore_case,
            },
            data: None,
        }
    }

    #[test]
    fn deepest_matching_category_wins_across_window_and_web_fields() {
        let classifier = ActivityClassifier::compile(&[
            class(&["Work"], "Chrome", false),
            class(&["Work", "Development"], "github\\.com", true),
        ])
        .unwrap();
        let data: Map<_, _> = [
            ("app".to_string(), json!("chrome.exe")),
            ("web_url".to_string(), json!("https://GitHub.com/project")),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            classifier.classify(&data).unwrap(),
            Some(vec!["Work".into(), "Development".into()])
        );
    }

    #[test]
    fn later_rule_wins_when_matching_categories_have_equal_depth() {
        let classifier = ActivityClassifier::compile(&[
            class(&["First"], "match", false),
            class(&["Second"], "match", false),
        ])
        .unwrap();
        let data: Map<_, _> = [("title".to_string(), json!("match"))]
            .into_iter()
            .collect();
        assert_eq!(
            classifier.classify(&data).unwrap(),
            Some(vec!["Second".into()])
        );
    }

    #[test]
    fn no_rule_match_is_uncategorized() {
        let classifier = ActivityClassifier::compile(&[class(&["Work"], "GitHub", false)]).unwrap();
        let data: Map<_, _> = [("title".to_string(), json!("Calendar"))]
            .into_iter()
            .collect();
        assert_eq!(classifier.classify(&data).unwrap(), None);
    }

    #[test]
    fn invalid_rule_is_rejected_instead_of_silently_misclassifying() {
        assert!(ActivityClassifier::compile(&[class(&["Work"], "(", false)]).is_err());
    }

    #[test]
    fn browser_bucket_must_match_the_active_window_process() {
        assert!(browser_bucket_matches_app(
            "aw-watcher-web-chrome",
            "chrome.exe"
        ));
        assert!(!browser_bucket_matches_app(
            "aw-watcher-web-firefox",
            "chrome.exe"
        ));
    }
}
