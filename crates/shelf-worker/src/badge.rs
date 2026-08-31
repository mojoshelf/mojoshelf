//! First-party status badges: flat shields-style SVGs rendered by the
//! worker from a tin's verification record, served at /badge/<tin>.svg
//! (stable) and /badge/<tin>/nightly.svg. No third-party badge service.

const GREEN: &str = "#4c1";
const RED: &str = "#e05d44";
const GREY: &str = "#9f9f9f";

/// Message + color for the stable (badge-bearing) verification.
pub fn stable_state(ok: Option<bool>, compiler: Option<&str>) -> (String, &'static str) {
    match ok {
        Some(true) => (
            match compiler {
                Some(c) => format!("verified · mojo {c}"),
                None => "verified".into(),
            },
            GREEN,
        ),
        Some(false) => ("failing".into(), RED),
        None => ("not verified".into(), GREY),
    }
}

/// Message + color for the nightly early-warning badge.
pub fn nightly_state(ok: Option<bool>, compiler: Option<&str>) -> (String, &'static str) {
    match ok {
        Some(true) => (
            match compiler {
                Some(c) => format!("passing · mojo {c}"),
                None => "passing".into(),
            },
            GREEN,
        ),
        Some(false) => ("failing".into(), RED),
        None => ("not checked".into(), GREY),
    }
}

/// Shields.io named color for the endpoint-schema JSON variant.
pub fn shields_color(hex: &str) -> &'static str {
    match hex {
        GREEN => "brightgreen",
        RED => "red",
        _ => "lightgrey",
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Verdana 11px averages ~6.6px per char; textLength below makes the exact
/// value non-critical (the renderer squeezes/stretches to fit).
fn text_width(s: &str) -> u32 {
    (s.chars().count() as f32 * 6.6).ceil() as u32
}

pub fn render(label: &str, message: &str, color: &str) -> String {
    let (label, message) = (xml_escape(label), xml_escape(message));
    let (ltw, mtw) = (text_width(&label), text_width(&message));
    let (lw, mw) = (ltw + 12, mtw + 12);
    let w = lw + mw;
    let lx = lw / 2;
    let mx = lw + mw / 2;
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="20" role="img" aria-label="{label}: {message}"><title>{label}: {message}</title><linearGradient id="s" x2="0" y2="100%"><stop offset="0" stop-color="#bbb" stop-opacity=".1"/><stop offset="1" stop-opacity=".1"/></linearGradient><clipPath id="r"><rect width="{w}" height="20" rx="3" fill="#fff"/></clipPath><g clip-path="url(#r)"><rect width="{lw}" height="20" fill="#555"/><rect x="{lw}" width="{mw}" height="20" fill="{color}"/><rect width="{w}" height="20" fill="url(#s)"/></g><g fill="#fff" text-anchor="middle" font-family="Verdana,Geneva,DejaVu Sans,sans-serif" font-size="11"><text x="{lx}" y="15" fill="#010101" fill-opacity=".3" textLength="{ltw}">{label}</text><text x="{lx}" y="14" textLength="{ltw}">{label}</text><text x="{mx}" y="15" fill="#010101" fill-opacity=".3" textLength="{mtw}">{message}</text><text x="{mx}" y="14" textLength="{mtw}">{message}</text></g></svg>"##
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_cover_pass_fail_unknown() {
        assert_eq!(
            stable_state(Some(true), Some("1.0.0")),
            ("verified · mojo 1.0.0".into(), GREEN)
        );
        assert_eq!(stable_state(Some(false), None), ("failing".into(), RED));
        assert_eq!(
            stable_state(None, Some("1.0.0")),
            ("not verified".into(), GREY)
        );
        assert_eq!(nightly_state(Some(true), None), ("passing".into(), GREEN));
        assert_eq!(nightly_state(None, None), ("not checked".into(), GREY));
    }

    #[test]
    fn svg_is_wellformed_and_escaped() {
        let svg = render("mojoshelf", "verified · mojo <1.0.0>", GREEN);
        assert!(svg.starts_with("<svg"));
        assert!(svg.ends_with("</svg>"));
        assert!(svg.contains("&lt;1.0.0&gt;"));
        assert!(!svg.contains("<1.0.0>"));
        assert!(svg.contains("#4c1"));
    }

    #[test]
    fn shields_colors_map() {
        assert_eq!(shields_color(GREEN), "brightgreen");
        assert_eq!(shields_color(RED), "red");
        assert_eq!(shields_color(GREY), "lightgrey");
    }
}
