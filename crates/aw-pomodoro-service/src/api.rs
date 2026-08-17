use std::time::Instant;

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};

use crate::model::{PomodoroSettings, StartRequest};
use crate::service::{PomodoroService, ServiceError};
use crate::{ActivityObservation, PomodoroEvent};

const MAX_API_BODY_SIZE: usize = 64 * 1024;

pub struct PomodoroApi {
    service: PomodoroService,
    allowed_origins: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct ApiRequest<'a> {
    pub method: &'a str,
    pub url: &'a str,
    pub origin: Option<&'a str>,
    pub content_type: Option<&'a str>,
    pub body: &'a [u8],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiResponse {
    pub status_code: u16,
    pub body: Vec<u8>,
    pub allow_origin: Option<String>,
}

impl PomodoroApi {
    pub fn new(service: PomodoroService, allowed_origins: Vec<String>) -> Self {
        Self {
            service,
            allowed_origins,
        }
    }

    pub fn handle(&mut self, request: ApiRequest<'_>) -> ApiResponse {
        self.handle_at(request, Instant::now(), Utc::now())
    }

    pub fn handle_at(
        &mut self,
        request: ApiRequest<'_>,
        now: Instant,
        wall_now: DateTime<Utc>,
    ) -> ApiResponse {
        let allow_origin = match self.check_origin(request.origin) {
            Ok(origin) => origin,
            Err(response) => return response,
        };

        let (path, query) = split_url(request.url);
        if request.method.eq_ignore_ascii_case("OPTIONS") {
            if path.starts_with("/pomodoro/") || path == "/pomodoro" {
                return ApiResponse {
                    status_code: 204,
                    body: Vec::new(),
                    allow_origin,
                };
            }
            return self.error(404, "not_found", "endpoint not found", allow_origin);
        }

        if request.body.len() > MAX_API_BODY_SIZE {
            return self.error(
                413,
                "body_too_large",
                "request body exceeds 64 KiB",
                allow_origin,
            );
        }

        if let Err(error) = self.service.tick(now, wall_now) {
            return self.service_error(error, allow_origin);
        }

        let result = match (request.method, path) {
            ("GET", "/pomodoro/state") => self.service.state(now).map(JsonBody::new),
            ("GET", "/pomodoro/settings") => Ok(JsonBody::new(self.service.settings())),
            ("PUT", "/pomodoro/settings") => {
                if let Err(response) = require_json_content_type(request, allow_origin.clone()) {
                    return response;
                }
                parse_json::<PomodoroSettings>(request.body)
                    .map_err(ServiceError::Validation)
                    .and_then(|settings| self.service.update_settings(settings))
                    .map(JsonBody::new)
            }
            ("POST", "/pomodoro/start") => {
                if let Err(response) = require_json_content_type(request, allow_origin.clone()) {
                    return response;
                }
                parse_json::<StartRequest>(request.body)
                    .map_err(ServiceError::Validation)
                    .and_then(|start| self.service.start(start, now, wall_now))
                    .map(JsonBody::new)
            }
            ("POST", "/pomodoro/pause") => {
                if let Err(response) = validate_command_request(request, allow_origin.clone()) {
                    return response;
                }
                self.service.pause(now, wall_now).map(JsonBody::new)
            }
            ("POST", "/pomodoro/resume") => {
                if let Err(response) = validate_command_request(request, allow_origin.clone()) {
                    return response;
                }
                self.service.resume(now, wall_now).map(JsonBody::new)
            }
            ("POST", "/pomodoro/stop") => {
                if let Err(response) = validate_command_request(request, allow_origin.clone()) {
                    return response;
                }
                self.service.stop(now, wall_now).map(JsonBody::new)
            }
            ("POST", "/pomodoro/confirm-next") => {
                if let Err(response) = validate_command_request(request, allow_origin.clone()) {
                    return response;
                }
                self.service.confirm_next(now, wall_now).map(JsonBody::new)
            }
            ("POST", "/pomodoro/distraction/continue") => {
                if let Err(response) = validate_command_request(request, allow_origin.clone()) {
                    return response;
                }
                self.service
                    .acknowledge_distraction(now, wall_now)
                    .map(JsonBody::new)
            }
            ("GET", "/pomodoro/history") => parse_history_query(query)
                .and_then(|(page, page_size)| self.service.history(page, page_size))
                .map(JsonBody::new),
            _ => {
                return self.error(404, "not_found", "endpoint not found", allow_origin);
            }
        };

        match result {
            Ok(body) => self.json(200, body.value, allow_origin),
            Err(error) => self.service_error(error, allow_origin),
        }
    }

    pub fn poll(&mut self) -> Result<(), ServiceError> {
        self.service.tick(Instant::now(), Utc::now())
    }

    pub fn observe_activity(
        &mut self,
        observation: ActivityObservation,
    ) -> Result<(), ServiceError> {
        self.observe_activity_at(observation, Instant::now(), Utc::now())
    }

    pub fn observe_activity_at(
        &mut self,
        observation: ActivityObservation,
        now: Instant,
        wall_now: DateTime<Utc>,
    ) -> Result<(), ServiceError> {
        self.service.observe_activity(observation, now, wall_now)
    }

    pub fn monitoring_failed(&mut self) -> Result<(), ServiceError> {
        self.monitoring_failed_at(Instant::now(), Utc::now())
    }

    pub fn monitoring_failed_at(
        &mut self,
        now: Instant,
        wall_now: DateTime<Utc>,
    ) -> Result<(), ServiceError> {
        self.service.monitoring_failed(now, wall_now)
    }

    pub fn should_monitor(&self) -> bool {
        self.service.should_monitor()
    }

    pub fn take_events(&mut self) -> Vec<PomodoroEvent> {
        self.service.take_events()
    }

    fn check_origin(&self, origin: Option<&str>) -> Result<Option<String>, ApiResponse> {
        let Some(origin) = origin else {
            return Ok(None);
        };
        if self.allowed_origins.iter().any(|allowed| allowed == origin) {
            return Ok(Some(origin.to_string()));
        }
        Err(self.error(
            403,
            "origin_forbidden",
            "request Origin is not allowed",
            None,
        ))
    }

    fn service_error(&self, error: ServiceError, allow_origin: Option<String>) -> ApiResponse {
        let (status, code) = match &error {
            ServiceError::Validation(_) => (400, "invalid_request"),
            ServiceError::Conflict(_) | ServiceError::Timer(_) => (409, "invalid_transition"),
            ServiceError::Storage(_) | ServiceError::History(_) | ServiceError::Internal(_) => {
                (500, "internal_error")
            }
        };
        self.error(status, code, &error.to_string(), allow_origin)
    }

    fn error(
        &self,
        status_code: u16,
        code: &str,
        message: &str,
        allow_origin: Option<String>,
    ) -> ApiResponse {
        self.json(
            status_code,
            json!({"error": {"code": code, "message": message}}),
            allow_origin,
        )
    }

    fn json(&self, status_code: u16, value: Value, allow_origin: Option<String>) -> ApiResponse {
        ApiResponse {
            status_code,
            body: serde_json::to_vec(&value).unwrap_or_else(|_| {
                br#"{"error":{"code":"internal_error","message":"serialization failed"}}"#.to_vec()
            }),
            allow_origin,
        }
    }
}

struct JsonBody {
    value: Value,
}

impl JsonBody {
    fn new(value: impl Serialize) -> Self {
        Self {
            value: serde_json::to_value(value).unwrap_or(Value::Null),
        }
    }
}

fn require_json_content_type(
    request: ApiRequest<'_>,
    allow_origin: Option<String>,
) -> Result<(), ApiResponse> {
    if request
        .content_type
        .is_some_and(|value| value.to_ascii_lowercase().starts_with("application/json"))
    {
        return Ok(());
    }
    Err(ApiResponse {
        status_code: 415,
        body: br#"{"error":{"code":"unsupported_media_type","message":"mutating requests require Content-Type: application/json"}}"#.to_vec(),
        allow_origin,
    })
}

fn validate_command_request(
    request: ApiRequest<'_>,
    allow_origin: Option<String>,
) -> Result<(), ApiResponse> {
    require_json_content_type(request, allow_origin.clone())?;
    if request.body.is_empty() {
        return Ok(());
    }
    match serde_json::from_slice::<Value>(request.body) {
        Ok(Value::Object(_)) => Ok(()),
        _ => Err(ApiResponse {
            status_code: 400,
            body: br#"{"error":{"code":"invalid_request","message":"command body must be a JSON object"}}"#.to_vec(),
            allow_origin,
        }),
    }
}

fn parse_json<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, String> {
    serde_json::from_slice(body).map_err(|error| format!("invalid JSON: {error}"))
}

fn split_url(url: &str) -> (&str, Option<&str>) {
    match url.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (url, None),
    }
}

fn parse_history_query(query: Option<&str>) -> Result<(u64, u64), ServiceError> {
    let mut page = 1;
    let mut page_size = 20;
    if let Some(query) = query {
        for pair in query.split('&').filter(|pair| !pair.is_empty()) {
            let (key, value) = pair.split_once('=').ok_or_else(|| {
                ServiceError::Validation("invalid history query string".to_string())
            })?;
            match key {
                "page" => {
                    page = value.parse().map_err(|_| {
                        ServiceError::Validation("page must be an integer".to_string())
                    })?;
                }
                "page_size" => {
                    page_size = value.parse().map_err(|_| {
                        ServiceError::Validation("page_size must be an integer".to_string())
                    })?;
                }
                _ => {
                    return Err(ServiceError::Validation(format!(
                        "unknown history query parameter: {key}"
                    )));
                }
            }
        }
    }
    Ok((page, page_size))
}
