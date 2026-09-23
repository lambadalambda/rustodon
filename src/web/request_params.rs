//! Form, JSON, multipart and Rack-style request parameter parsing.

use super::*;

#[derive(Clone)]
pub(super) struct BufferedRequestBody(pub(super) Bytes);

#[derive(Clone, Copy, Debug)]
pub(super) enum RequestParameterError {
    BadRequest,
    InternalServer,
}

pub(super) fn merge_request_parameters(request: &mut Request) -> Result<(), RequestParameterError> {
    let query_parameters = RackParameters::parse(request.uri().query().unwrap_or_default())
        .map_err(RequestParameterError::from)?;
    let body = request.extensions().get::<BufferedRequestBody>();
    let content_type = request
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::trim);
    let body_parameters = if body.is_none_or(|body| body.0.is_empty()) {
        RackParameters::default()
    } else {
        let Some(body) = body else {
            unreachable!("nonempty request body is present")
        };
        match content_type {
            Some(value) if content_type_is(value, "application/x-www-form-urlencoded") => {
                let body =
                    std::str::from_utf8(&body.0).map_err(|_| RequestParameterError::BadRequest)?;
                if !valid_query(body) {
                    return Err(RequestParameterError::BadRequest);
                }
                RackParameters::parse(body).map_err(RequestParameterError::from)?
            }
            Some(value) if content_type_is(value, "multipart/form-data") => {
                parse_multipart_parameters(&body.0, value)?
            }
            Some(value)
                if json_content_type(value.split(';').next().unwrap_or_default().trim()) =>
            {
                let value = serde_json::from_slice(&body.0)
                    .map_err(|_| RequestParameterError::BadRequest)?;
                let root = RackValue::from_json(&value);
                if root.json_depth() > 100 {
                    return Err(RequestParameterError::BadRequest);
                }
                RackParameters::from_json(&value)
            }
            _ => RackParameters::default(),
        }
    };
    let merged = body_parameters.merge(query_parameters);
    let query = merged.to_query();
    let path_and_query = if query.is_empty() {
        request.uri().path().to_owned()
    } else {
        format!("{}?{query}", request.uri().path())
    };
    *request.uri_mut() = path_and_query
        .parse()
        .map_err(|_| RequestParameterError::BadRequest)?;
    request.extensions_mut().insert(merged);
    Ok(())
}

pub(super) fn json_content_type(value: &str) -> bool {
    [
        "application/json",
        "text/x-json",
        "application/jsonrequest",
        "application/jrd+json",
        "application/activity+json",
        "application/ld+json",
        "application/problem+json",
    ]
    .iter()
    .any(|mime| value.eq_ignore_ascii_case(mime))
}

pub(super) fn content_type_is(value: &str, expected: &str) -> bool {
    value
        .split(';')
        .next()
        .is_some_and(|value| value.trim().eq_ignore_ascii_case(expected))
}

pub(super) fn parse_multipart_parameters(
    body: &[u8],
    content_type: &str,
) -> Result<RackParameters, RequestParameterError> {
    let boundary = multipart_boundary(content_type).ok_or(RequestParameterError::BadRequest)?;
    let marker = format!("--{boundary}");
    let marker = marker.as_bytes();
    if !body.starts_with(marker) {
        return Err(RequestParameterError::BadRequest);
    }
    let separator = format!("\r\n--{boundary}");
    let mut offset = marker.len();
    let mut parameters = RackParameters::default();
    let mut count = 0_usize;
    loop {
        if body.get(offset..offset + 2) == Some(b"--") {
            break;
        }
        if body.get(offset..offset + 2) != Some(b"\r\n") {
            return Err(RequestParameterError::BadRequest);
        }
        offset += 2;
        let Some(header_end) = find_bytes(&body[offset..], b"\r\n\r\n") else {
            return Err(RequestParameterError::BadRequest);
        };
        let headers = parse_multipart_headers(&body[offset..offset + header_end])?;
        offset += header_end + 4;
        let relative = find_bytes(&body[offset..], separator.as_bytes())
            .ok_or(RequestParameterError::BadRequest)?;
        let next_marker = offset + relative;
        let value = &body[offset..next_marker];
        let name = headers
            .content_disposition
            .name
            .as_deref()
            .ok_or(RequestParameterError::BadRequest)?;
        let Some((root, segments)) = rack_key(name).map_err(RequestParameterError::from)? else {
            return Err(RequestParameterError::BadRequest);
        };
        let value = if let Some(file_name) = headers.content_disposition.file_name {
            if file_name.is_empty() {
                RackValue::Null
            } else {
                if file_name.contains(['\0', '\r', '\n']) {
                    return Err(RequestParameterError::BadRequest);
                }
                if matches!(name, "avatar" | "header") && value.len() >= 8 * 1024 * 1024 {
                    return Err(RequestParameterError::BadRequest);
                }
                RackValue::Upload(UploadedFile {
                    file_name,
                    content_type: headers
                        .content_type
                        .unwrap_or_else(|| "application/octet-stream".to_owned()),
                    bytes: value.to_vec(),
                })
            }
        } else {
            RackValue::Scalar(
                String::from_utf8(value.to_vec()).map_err(|_| RequestParameterError::BadRequest)?,
            )
        };
        count += 1;
        if count > RACK_PARAMETER_LIMIT {
            return Err(RequestParameterError::InternalServer);
        }
        parameters
            .insert(root, &segments, value)
            .map_err(RequestParameterError::from)?;
        offset = next_marker + 2 + marker.len();
    }
    if body.get(offset + 2..) != Some(b"\r\n") {
        return Err(RequestParameterError::BadRequest);
    }
    Ok(parameters)
}

#[derive(Clone, Debug)]
pub(super) struct UploadedFile {
    pub(super) file_name: String,
    pub(super) content_type: String,
    pub(super) bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub(super) struct MultipartHeaders {
    pub(super) content_disposition: MultipartContentDisposition,
    pub(super) content_type: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct MultipartContentDisposition {
    pub(super) name: Option<String>,
    pub(super) file_name: Option<String>,
}

pub(super) fn multipart_boundary(content_type: &str) -> Option<String> {
    if !content_type_is(content_type, "multipart/form-data") {
        return None;
    }
    content_type
        .split(';')
        .skip(1)
        .filter_map(|parameter| parameter.trim().split_once('='))
        .find_map(|(key, value)| {
            if !key.trim().eq_ignore_ascii_case("boundary") {
                return None;
            }
            let value = value.trim();
            let value = value
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .unwrap_or(value);
            (!value.is_empty()
                && value.len() <= 70
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_graphic() && byte != b'"'))
            .then(|| value.to_owned())
        })
}

pub(super) fn parse_multipart_headers(
    value: &[u8],
) -> Result<MultipartHeaders, RequestParameterError> {
    let mut content_disposition = None;
    let mut content_type = None;
    for line in value.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            return Err(RequestParameterError::BadRequest);
        }
        let Some(separator) = line.iter().position(|byte| *byte == b':') else {
            return Err(RequestParameterError::BadRequest);
        };
        let (name, value) = line.split_at(separator);
        let value = &value[1..];
        let name = std::str::from_utf8(name)
            .map_err(|_| RequestParameterError::BadRequest)?
            .trim();
        let value = std::str::from_utf8(value)
            .map_err(|_| RequestParameterError::BadRequest)?
            .trim();
        if name.eq_ignore_ascii_case("content-disposition") {
            content_disposition = Some(parse_content_disposition(value)?);
        } else if name.eq_ignore_ascii_case("content-type") {
            content_type = Some(value.to_owned());
        }
    }
    Ok(MultipartHeaders {
        content_disposition: content_disposition.ok_or(RequestParameterError::BadRequest)?,
        content_type,
    })
}

pub(super) fn parse_content_disposition(
    value: &str,
) -> Result<MultipartContentDisposition, RequestParameterError> {
    let mut disposition = value.split(';');
    if !disposition
        .next()
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("form-data"))
    {
        return Err(RequestParameterError::BadRequest);
    }
    let mut result = MultipartContentDisposition::default();
    for parameter in disposition {
        let Some((key, value)) = parameter.trim().split_once('=') else {
            return Err(RequestParameterError::BadRequest);
        };
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .ok_or(RequestParameterError::BadRequest)?;
        match key.trim().to_ascii_lowercase().as_str() {
            "name" => result.name = Some(value.to_owned()),
            "filename" => result.file_name = Some(value.to_owned()),
            _ => {}
        }
    }
    Ok(result)
}

pub(super) fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[derive(Clone, Debug)]
pub(super) enum RackValue {
    Null,
    Scalar(String),
    Number(serde_json::Number),
    Boolean(bool),
    Array(Vec<RackValue>),
    Object(BTreeMap<String, RackValue>),
    Upload(UploadedFile),
}

#[derive(Clone, Debug, Default)]
pub(super) struct RackParameters(pub(super) BTreeMap<String, RackValue>);

#[derive(Clone, Debug)]
pub(super) enum RackKeySegment {
    Field(String),
    TrailingPush(String),
    Push,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum RackParseError {
    Conflict,
    Limit,
}

impl From<RackParseError> for RequestParameterError {
    fn from(error: RackParseError) -> Self {
        match error {
            RackParseError::Conflict => Self::BadRequest,
            RackParseError::Limit => Self::InternalServer,
        }
    }
}

impl RackParameters {
    pub(super) fn parse(encoded: &str) -> Result<Self, RackParseError> {
        if encoded.len() > RACK_BYTES_LIMIT {
            return Err(RackParseError::Limit);
        }
        let mut parameters = Self::default();
        for (index, raw) in encoded.split('&').enumerate() {
            if index == RACK_PARAMETER_LIMIT {
                return Err(RackParseError::Limit);
            }
            let Some((name, value)) = url::form_urlencoded::parse(raw.as_bytes()).next() else {
                continue;
            };
            let Some((root, segments)) = rack_key(&name)? else {
                continue;
            };
            let value = if raw.contains('=') {
                RackValue::Scalar(value.into_owned())
            } else {
                RackValue::Null
            };
            parameters.insert(root, &segments, value)?;
        }
        Ok(parameters)
    }

    pub(super) fn from_json(value: &serde_json::Value) -> Self {
        let serde_json::Value::Object(values) = value else {
            return Self(BTreeMap::from([(
                "_json".to_owned(),
                RackValue::from_json(value),
            )]));
        };
        Self(
            values
                .iter()
                .map(|(key, value)| (key.clone(), RackValue::from_json(value)))
                .collect(),
        )
    }

    pub(super) fn insert(
        &mut self,
        root: String,
        segments: &[RackKeySegment],
        value: RackValue,
    ) -> Result<(), RackParseError> {
        if segments.is_empty() {
            self.0.insert(root, value);
            return Ok(());
        }
        let expected = match segments[0] {
            RackKeySegment::Field(_) => RackValue::Object(BTreeMap::new()),
            RackKeySegment::Push | RackKeySegment::TrailingPush(_) => RackValue::Array(Vec::new()),
        };
        insert_rack_value(self.0.entry(root).or_insert(expected), segments, value)
    }

    pub(super) fn merge(mut self, query: Self) -> Self {
        self.0.extend(query.0);
        self
    }

    pub(super) fn get(&self, name: &str) -> Option<&RackValue> {
        self.0.get(name)
    }

    pub(super) fn to_query(&self) -> String {
        fn append(pairs: &mut Vec<(String, String)>, name: String, value: &RackValue) {
            match value {
                RackValue::Null | RackValue::Upload(_) => {}
                RackValue::Scalar(value) => pairs.push((name, value.clone())),
                RackValue::Number(value) => pairs.push((name, ruby_json_number(value))),
                RackValue::Boolean(value) => pairs.push((name, value.to_string())),
                RackValue::Array(values) => {
                    for value in values {
                        append(pairs, format!("{name}[]"), value);
                    }
                }
                RackValue::Object(values) => {
                    for (key, value) in values {
                        append(pairs, format!("{name}[{key}]"), value);
                    }
                }
            }
        }

        let mut pairs = Vec::new();
        for (name, value) in &self.0 {
            append(&mut pairs, name.clone(), value);
        }
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        serializer.extend_pairs(pairs);
        serializer.finish()
    }
}

impl RackValue {
    pub(super) fn from_json(value: &serde_json::Value) -> Self {
        match value {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::String(value) => Self::Scalar(value.clone()),
            serde_json::Value::Number(value) => Self::Number(value.clone()),
            serde_json::Value::Bool(value) => Self::Boolean(*value),
            serde_json::Value::Array(values) => {
                Self::Array(values.iter().map(Self::from_json).collect())
            }
            serde_json::Value::Object(values) => Self::Object(
                values
                    .iter()
                    .map(|(key, value)| (key.clone(), Self::from_json(value)))
                    .collect(),
            ),
        }
    }

    pub(super) fn json_depth(&self) -> usize {
        match self {
            Self::Array(values) => 1 + values.iter().map(Self::json_depth).max().unwrap_or(0),
            Self::Object(values) => 1 + values.values().map(Self::json_depth).max().unwrap_or(0),
            Self::Null | Self::Scalar(_) | Self::Number(_) | Self::Boolean(_) | Self::Upload(_) => {
                0
            }
        }
    }
}

pub(super) fn rack_key(
    name: &str,
) -> Result<Option<(String, Vec<RackKeySegment>)>, RackParseError> {
    let root_end = name
        .get(1..)
        .and_then(|suffix| suffix.find('[').map(|index| index + 1))
        .unwrap_or(name.len());
    let root = &name[..root_end];
    if root.is_empty() {
        return Ok(None);
    }
    let mut segments = Vec::new();
    let mut suffix = &name[root_end..];
    while !suffix.is_empty() {
        let Some(rest) = suffix.strip_prefix('[') else {
            break;
        };
        let end = rest.find(']').unwrap_or(rest.len());
        let field = &rest[..end];
        let remaining = if end == rest.len() {
            ""
        } else {
            &rest[end + 1..]
        };
        segments.push(if field.is_empty() {
            if !remaining.is_empty() && matches!(segments.last(), Some(RackKeySegment::Push)) {
                RackKeySegment::Field("[]".to_owned())
            } else {
                RackKeySegment::Push
            }
        } else {
            RackKeySegment::Field(field.to_owned())
        });
        suffix = remaining;
        if segments.len() >= RACK_DEPTH_LIMIT {
            return Err(RackParseError::Limit);
        }
    }
    if !suffix.is_empty() {
        match segments.last() {
            Some(RackKeySegment::Field(_)) => {
                segments.push(RackKeySegment::Field(suffix.to_owned()));
            }
            Some(RackKeySegment::Push) => {
                segments.pop();
                segments.push(RackKeySegment::TrailingPush(suffix.to_owned()));
            }
            Some(RackKeySegment::TrailingPush(_)) | None => {}
        }
    }
    if segments.len() >= RACK_DEPTH_LIMIT {
        return Err(RackParseError::Limit);
    }
    Ok(Some((root.to_owned(), segments)))
}

pub(super) fn insert_rack_value(
    target: &mut RackValue,
    segments: &[RackKeySegment],
    value: RackValue,
) -> Result<(), RackParseError> {
    let Some((segment, rest)) = segments.split_first() else {
        *target = value;
        return Ok(());
    };
    match (target, segment) {
        (RackValue::Object(values), RackKeySegment::Field(field)) => {
            if rest.is_empty() {
                values.insert(field.clone(), value);
                return Ok(());
            }
            let expected = match rest[0] {
                RackKeySegment::Field(_) => RackValue::Object(BTreeMap::new()),
                RackKeySegment::Push | RackKeySegment::TrailingPush(_) => {
                    RackValue::Array(Vec::new())
                }
            };
            insert_rack_value(values.entry(field.clone()).or_insert(expected), rest, value)
        }
        (RackValue::Array(values), RackKeySegment::TrailingPush(field)) => {
            let path = [RackKeySegment::Field(field.clone())];
            if let Some(RackValue::Object(last)) = values.last()
                && !matches!(rack_path_state(last, &path), RackPathState::Existing)
            {
                let last = values.last_mut().expect("last array object exists");
                return insert_rack_value(last, &path, value);
            }
            let mut nested = RackValue::Object(BTreeMap::new());
            insert_rack_value(&mut nested, &path, value)?;
            values.push(nested);
            Ok(())
        }
        (RackValue::Array(values), RackKeySegment::Push) => {
            if rest.is_empty() {
                values.push(value);
                return Ok(());
            }
            if matches!(rest.first(), Some(RackKeySegment::Field(_)))
                && let Some(RackValue::Object(last)) = values.last()
                && !matches!(rack_path_state(last, rest), RackPathState::Existing)
            {
                let last = values.last_mut().expect("last array object exists");
                return insert_rack_value(last, rest, value);
            }
            if matches!(rest, [RackKeySegment::Push])
                && matches!(values.last(), Some(RackValue::Object(_)))
            {
                return Ok(());
            }
            if matches!(rest.first(), Some(RackKeySegment::Push))
                && matches!(values.last(), Some(RackValue::Array(_)))
            {
                let last = values.last_mut().expect("last nested array exists");
                return insert_rack_value(last, rest, value);
            }
            let mut nested = match rest[0] {
                RackKeySegment::Field(_) => RackValue::Object(BTreeMap::new()),
                RackKeySegment::Push | RackKeySegment::TrailingPush(_) => {
                    RackValue::Array(Vec::new())
                }
            };
            insert_rack_value(&mut nested, rest, value)?;
            values.push(nested);
            Ok(())
        }
        _ => Err(RackParseError::Conflict),
    }
}

#[derive(Clone, Copy)]
pub(super) enum RackPathState {
    Missing,
    Existing,
    Conflict,
}

pub(super) fn rack_path_state(
    values: &BTreeMap<String, RackValue>,
    segments: &[RackKeySegment],
) -> RackPathState {
    let Some((segment, rest)) = segments.split_first() else {
        return RackPathState::Existing;
    };
    let RackKeySegment::Field(field) = segment else {
        return RackPathState::Conflict;
    };
    let Some(value) = values.get(field) else {
        return RackPathState::Missing;
    };
    let Some((next, _)) = rest.split_first() else {
        return RackPathState::Existing;
    };
    match (value, next) {
        (RackValue::Object(values), RackKeySegment::Field(_)) => rack_path_state(values, rest),
        (RackValue::Array(_), RackKeySegment::Push | RackKeySegment::TrailingPush(_)) => {
            RackPathState::Missing
        }
        _ => RackPathState::Conflict,
    }
}
