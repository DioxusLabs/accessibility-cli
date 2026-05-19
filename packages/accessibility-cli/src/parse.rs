use accessibility_core::input::MouseButton;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointArg {
    pub x: f64,
    pub y: f64,
}

impl PointArg {
    pub fn tuple(self) -> (f64, f64) {
        (self.x, self.y)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SwipeArg {
    pub start: (f64, f64),
    pub end: (f64, f64),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SwipeParams {
    pub start: (f64, f64),
    pub end: (f64, f64),
    pub duration_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MouseClickParams {
    pub x: f64,
    pub y: f64,
    pub button: MouseButton,
}

pub fn parse_point(s: &str) -> Result<PointArg, String> {
    let parts = split_exact(s, 2, "Expected format: x,y")?;
    Ok(PointArg {
        x: parse_f64(parts[0], "Invalid x coordinate")?,
        y: parse_f64(parts[1], "Invalid y coordinate")?,
    })
}

pub fn parse_swipe(s: &str) -> Result<SwipeArg, String> {
    let parts = split_exact(s, 4, "Expected format: x1,y1,x2,y2")?;
    Ok(SwipeArg {
        start: (
            parse_f64(parts[0], "Invalid x1")?,
            parse_f64(parts[1], "Invalid y1")?,
        ),
        end: (
            parse_f64(parts[2], "Invalid x2")?,
            parse_f64(parts[3], "Invalid y2")?,
        ),
    })
}

fn split_exact<'a>(s: &'a str, count: usize, message: &str) -> Result<Vec<&'a str>, String> {
    let parts = s.split(',').map(str::trim).collect::<Vec<_>>();
    if parts.len() != count || parts.iter().any(|part| part.is_empty()) {
        return Err(message.to_string());
    }
    Ok(parts)
}

fn parse_f64(s: &str, message: &str) -> Result<f64, String> {
    s.parse::<f64>().map_err(|_| message.to_string())
}
