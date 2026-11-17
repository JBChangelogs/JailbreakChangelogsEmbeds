use anyhow::{Context, Result};
use headless_chrome::Tab;

use crate::{browser::goto, render::escape};

const DEFAULT_ACCENT: u32 = 0x1d7cf2;

pub fn profile_card(tab: &Tab, id: &str) -> Result<String> {
    anyhow::ensure!(goto(tab, &format!("https://api.jailbreakchangelogs.com/v2/users/{id}"))?, "user API didn't load");
    let body = tab.evaluate("document.body.innerText", false)?.value.context("empty API response")?;
    let u: serde_json::Value = serde_json::from_str(body.as_str().unwrap_or_default()).context("parsing user JSON")?;
    if !u.is_object() || u.get("detail").is_some() {
        eprintln!("  no profile card for {id}: {u}");
        return Ok(String::new());
    }

    let field = |k: &str| u[k].as_str().filter(|s| !s.is_empty() && *s != "None").map(str::to_string);
    let color = |k: &str| u[k].as_u64().or_else(|| u[k].as_str()?.parse().ok()).map(|c| c as u32 & 0xffffff);
    let username = field("username").with_context(|| format!("no user {id}"))?;
    let name = field("global_name").unwrap_or_else(|| username.clone());
    let avatar = field("custom_avatar")
        .filter(|_| u["settings_v2"]["custom_avatar"].as_bool() == Some(true))
        .or_else(|| field("avatar").map(|a| format!("https://cdn.discordapp.com/avatars/{id}/{a}.png?size=512")))
        .unwrap_or_else(|| "https://cdn.discordapp.com/embed/avatars/0.png".into());

    let accents: Vec<u32> = match (color("accent_color"), color("accent_gradient")) {
        (Some(a), Some(b)) => vec![a, b],
        (Some(a), None) => vec![a],
        _ => vec![DEFAULT_ACCENT],
    };
    let background = match accents.as_slice() {
        [a, b] => format!("linear-gradient(135deg, {}, {})", hex(*a), hex(*b)),
        _ => hex(accents[0]),
    };
    let badge = u["usernumber"]
        .as_u64()
        .map(|n| format!("<span class=\"pill\" style=\"background:{background};color:{}\">User #{n}</span>", text_color(&accents)))
        .unwrap_or_default();
    let glow = color("underglow").map(|c| format!(" style=\"--underglow:{}\"", hex(c))).unwrap_or_default();

    Ok(format!(
        "<div class=\"card\"{glow}><img class=\"avatar\" src=\"{}\"><div><div class=\"name\">{}</div><div class=\"handle\">@{}</div>{badge}</div></div>",
        escape(&avatar),
        escape(&name),
        escape(&username)
    ))
}

fn hex(c: u32) -> String {
    format!("#{c:06x}")
}

fn text_color(backgrounds: &[u32]) -> &'static str {
    let worst = |text: u32| backgrounds.iter().map(|&b| contrast(text, b)).fold(f64::INFINITY, f64::min);
    if worst(0xffffff) >= 3.0 || worst(0xffffff) >= worst(0x0b0b0f) { "#fff" } else { "#0b0b0f" }
}

fn contrast(a: u32, b: u32) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

fn luminance(c: u32) -> f64 {
    let channel = |shift: u32| {
        let v = ((c >> shift) & 0xff) as f64 / 255.0;
        if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
}

#[cfg(test)]
mod tests {
    use super::text_color;

    #[test]
    fn picks_readable_text() {
        assert_eq!(text_color(&[0x1d7cf2]), "#fff");
        assert_eq!(text_color(&[0xa8b2b9]), "#0b0b0f");
        assert_eq!(text_color(&[0x101014]), "#fff");
        assert_eq!(text_color(&[0xffff00]), "#0b0b0f");
        assert_eq!(text_color(&[0xa8b2b9, 0x6f79bf]), "#0b0b0f");
    }
}
