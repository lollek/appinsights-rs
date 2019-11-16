use std::fmt::{Display, Formatter};
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use http::{Method, StatusCode, Uri};
use uuid::Uuid;

use crate::context::TelemetryContext;
use crate::contracts::*;
use crate::telemetry::{ContextTags, Measurements, Properties, Telemetry};
use crate::time;

// Represents completion of an external request to the application and contains a summary of that
// request execution and results.
pub struct RequestTelemetry {
    // Identifier of a request call instance. It is used for correlation between request
    // and other telemetry items.
    id: Uuid,

    // Request name. For HTTP requests it represents the HTTP method and URL path template.
    name: String,

    // URL of the request with all query string parameters.
    uri: Uri,

    // Duration to serve the request.
    duration: FormattedDuration,

    // Results of a request execution. HTTP status code for HTTP requests.
    response_code: StatusCode,

    /// The time stamp when this telemetry was measured.
    timestamp: DateTime<Utc>,

    /// Custom properties.
    properties: Properties,

    /// Telemetry context containing extra, optional tags.
    tags: ContextTags,

    /// Custom measurements.
    measurements: Measurements,
}

impl RequestTelemetry {
    /// Creates a new telemetry item for HTTP request.
    pub fn new(method: Method, uri: Uri, duration: Duration, response_code: impl Into<StatusCode>) -> Self {
        let mut authority = String::new();
        if let Some(host) = &uri.host() {
            authority.push_str(host);
        }
        if let Some(port) = &uri.port_u16() {
            authority.push_str(&format!(":{}", port))
        }

        let uri = Uri::builder()
            .scheme(uri.scheme_str().unwrap_or_default())
            .authority(authority.as_str())
            .path_and_query(uri.path())
            .build()
            .unwrap_or(uri);

        Self {
            id: id::new(),
            name: format!("{} {}", method, uri),
            uri,
            duration: FormattedDuration(duration),
            response_code: response_code.into(),
            timestamp: time::now(),
            properties: Default::default(),
            tags: Default::default(),
            measurements: Default::default(),
        }
    }

    /// Returns custom measurements to submit with the telemetry item.
    pub fn measurements(&self) -> &Measurements {
        &self.measurements
    }

    /// Returns mutable reference to custom measurements.
    pub fn measurements_mut(&mut self) -> &mut Measurements {
        &mut self.measurements
    }

    // Returns an indication of successful or unsuccessful call.
    pub fn is_success(&self) -> bool {
        self.response_code < StatusCode::BAD_REQUEST || self.response_code == StatusCode::UNAUTHORIZED
    }
}

impl Telemetry for RequestTelemetry {
    /// Returns the time when this telemetry was measured.
    fn timestamp(&self) -> DateTime<Utc> {
        self.timestamp
    }

    /// Returns custom properties to submit with the telemetry item.
    fn properties(&self) -> &Properties {
        &self.properties
    }

    /// Returns mutable reference to custom properties.
    fn properties_mut(&mut self) -> &mut Properties {
        &mut self.properties
    }

    /// Returns context data containing extra, optional tags. Overrides values found on client telemetry context.
    fn tags(&self) -> &ContextTags {
        &self.tags
    }

    /// Returns mutable reference to custom tags.
    fn tags_mut(&mut self) -> &mut ContextTags {
        &mut self.tags
    }
}

impl From<(TelemetryContext, RequestTelemetry)> for Envelope {
    fn from((context, telemetry): (TelemetryContext, RequestTelemetry)) -> Self {
        let success = telemetry.is_success();
        let data = Data::RequestData(
            RequestDataBuilder::new(
                telemetry.id.to_hyphenated().to_string(),
                telemetry.duration.to_string(),
                telemetry.response_code.as_str(),
            )
            .name(telemetry.name)
            .success(success)
            .url(telemetry.uri.to_string())
            .properties(Properties::combine(context.properties, telemetry.properties))
            .measurements(telemetry.measurements)
            .build(),
        );

        let envelope_name = data.envelope_name(&context.normalized_i_key);
        let timestamp = telemetry.timestamp.to_rfc3339_opts(SecondsFormat::Millis, true);

        EnvelopeBuilder::new(envelope_name, timestamp)
            .data(Base::Data(data))
            .i_key(context.i_key)
            .tags(ContextTags::combine(context.tags, telemetry.tags))
            .build()
    }
}

/// Provides dotnet duration aware formatting rules.
struct FormattedDuration(Duration);

impl Display for FormattedDuration {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let nanoseconds = self.0.as_nanos();
        let ticks = nanoseconds / 100 % 10_000_000;
        let total_seconds = nanoseconds / 1_000_000_000;
        let seconds = total_seconds % 60;
        let minutes = total_seconds / 60 % 60;
        let hours = total_seconds / 3600 % 24;
        let days = total_seconds / 86400;

        write!(
            f,
            "{}.{:0>2}:{:0>2}:{:0>2}.{:0>7}",
            days, hours, minutes, seconds, ticks
        )
    }
}

#[cfg(not(test))]
mod id {
    use uuid::Uuid;

    pub fn new() -> Uuid {
        Uuid::new_v4()
    }
}

#[cfg(test)]
mod id {
    use std::cell::RefCell;

    use uuid::Uuid;

    thread_local!(static ID: RefCell<Option<Uuid>> = RefCell::new(None));

    pub fn new() -> Uuid {
        ID.with(|is| {
            if let Some(id) = *is.borrow() {
                id
            } else {
                Uuid::new_v4()
            }
        })
    }

    pub fn set(uuid: Uuid) {
        ID.with(|is| *is.borrow_mut() = Some(uuid))
    }

    pub fn reset() {
        ID.with(|is| *is.borrow_mut() = None)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::str::FromStr;

    use chrono::TimeZone;
    use test_case::test_case;

    use super::*;

    #[test]
    fn it_overrides_properties_from_context() {
        time::set(Utc.ymd(2019, 1, 2).and_hms_milli(3, 4, 5, 800));
        id::set(Uuid::from_str("910b414a-f368-4b3a-aff6-326632aac566").unwrap());

        let mut context = TelemetryContext::new("instrumentation".into());
        context.properties_mut().insert("test".into(), "ok".into());
        context.properties_mut().insert("no-write".into(), "fail".into());

        let mut telemetry = RequestTelemetry::new(
            Method::GET,
            "https://example.com/main.html".parse().unwrap(),
            Duration::from_secs(2),
            StatusCode::OK,
        );
        telemetry.properties_mut().insert("no-write".into(), "ok".into());
        telemetry.measurements_mut().insert("latency".into(), 200.0);

        let envelop = Envelope::from((context, telemetry));

        let expected = EnvelopeBuilder::new(
            "Microsoft.ApplicationInsights.instrumentation.Request",
            "2019-01-02T03:04:05.800Z",
        )
        .data(Base::Data(Data::RequestData(
            RequestDataBuilder::new("910b414a-f368-4b3a-aff6-326632aac566", "0.00:00:02.0000000", "200")
                .name("GET https://example.com/main.html")
                .success(true)
                .url("https://example.com/main.html")
                .properties({
                    let mut properties = BTreeMap::default();
                    properties.insert("test".into(), "ok".into());
                    properties.insert("no-write".into(), "ok".into());
                    properties
                })
                .measurements({
                    let mut measurement = Measurements::default();
                    measurement.insert("latency".into(), 200.0);
                    measurement
                })
                .build(),
        )))
        .i_key("instrumentation")
        .tags(BTreeMap::default())
        .build();

        assert_eq!(envelop, expected)
    }

    #[test]
    fn it_overrides_tags_from_context() {
        time::set(Utc.ymd(2019, 1, 2).and_hms_milli(3, 4, 5, 700));
        id::set(Uuid::from_str("910b414a-f368-4b3a-aff6-326632aac566").unwrap());

        let mut context = TelemetryContext::new("instrumentation".into());
        context.tags_mut().insert("test".into(), "ok".into());
        context.tags_mut().insert("no-write".into(), "fail".into());

        let mut telemetry = RequestTelemetry::new(
            Method::GET,
            "https://example.com/main.html".parse().unwrap(),
            Duration::from_secs(2),
            StatusCode::OK,
        );
        telemetry.measurements_mut().insert("latency".into(), 200.0);
        telemetry.tags_mut().insert("no-write".into(), "ok".into());

        let envelop = Envelope::from((context, telemetry));

        let expected = EnvelopeBuilder::new(
            "Microsoft.ApplicationInsights.instrumentation.Request",
            "2019-01-02T03:04:05.700Z",
        )
        .data(Base::Data(Data::RequestData(
            RequestDataBuilder::new("910b414a-f368-4b3a-aff6-326632aac566", "0.00:00:02.0000000", "200")
                .name("GET https://example.com/main.html")
                .success(true)
                .url("https://example.com/main.html")
                .properties(Properties::default())
                .measurements({
                    let mut measurement = Measurements::default();
                    measurement.insert("latency".into(), 200.0);
                    measurement
                })
                .build(),
        )))
        .i_key("instrumentation")
        .tags({
            let mut tags = BTreeMap::default();
            tags.insert("test".into(), "ok".into());
            tags.insert("no-write".into(), "ok".into());
            tags
        })
        .build();

        assert_eq!(envelop, expected)
    }

    #[test_case(Duration::from_secs(3600),  "0.01:00:00.0000000"    ; "hour")]
    #[test_case(Duration::from_secs(60),    "0.00:01:00.0000000"    ; "minute")]
    #[test_case(Duration::from_secs(1),     "0.00:00:01.0000000"    ; "second")]
    #[test_case(Duration::from_millis(1),   "0.00:00:00.0010000"    ; "millisecond")]
    #[test_case(Duration::from_nanos(100),  "0.00:00:00.0000001"    ; "tick")]
    #[test_case((Utc.ymd(2019, 1, 3).and_hms(1, 2, 3) - Utc.ymd(2019, 1, 1).and_hms(0, 0, 0)).to_std().unwrap(), "2.01:02:03.0000000"    ; "custom")]
    fn it_converts_duration_to_string(duration: Duration, expected: &'static str) {
        assert_eq!(FormattedDuration(duration).to_string(), expected.to_string())
    }
}
