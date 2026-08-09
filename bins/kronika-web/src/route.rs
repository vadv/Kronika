//! What a request asks for: the path, and the window it names.

/// The requests the server answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Route {
    /// The health line over a window.
    Health,
}

/// The window a request names, in unix microseconds.
///
/// An absent bound means everything on that side. The server does not invent
/// one from the clock: a request that wants the last hour says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Window {
    /// Oldest timestamp to include.
    pub(crate) from: Option<i64>,
    /// Newest timestamp to include.
    pub(crate) to: Option<i64>,
}

/// Why a request was refused before it reached the data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RouteError {
    /// No handler answers that path.
    NoSuchPath,
    /// A parameter is present and unusable.
    BadParameter(String),
}

impl std::fmt::Display for RouteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSuchPath => write!(f, "no such path"),
            Self::BadParameter(name) => write!(f, "{name} is not a number"),
        }
    }
}

/// Read the route and its window out of a request target.
///
/// # Errors
///
/// Returns [`RouteError::NoSuchPath`] for a path nothing answers, and
/// [`RouteError::BadParameter`] for a bound that is not a number.
pub(crate) fn parse(target: &str) -> Result<(Route, Window), RouteError> {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let route = match path {
        "/api/health" => Route::Health,
        _other => return Err(RouteError::NoSuchPath),
    };
    let mut window = Window::default();
    for (name, value) in pairs(query) {
        match name {
            "from" => window.from = Some(number("from", value)?),
            "to" => window.to = Some(number("to", value)?),
            _other => {}
        }
    }
    Ok((route, window))
}

/// The `name=value` pairs of a query string, in the order they were written.
fn pairs(query: &str) -> impl Iterator<Item = (&str, &str)> {
    query
        .split('&')
        .filter(|part| !part.is_empty())
        .map(|part| part.split_once('=').unwrap_or((part, "")))
}

fn number(name: &str, value: &str) -> Result<i64, RouteError> {
    value
        .parse()
        .map_err(|_reason| RouteError::BadParameter(name.to_owned()))
}

#[cfg(test)]
mod tests;
