use anyhow::{Context, Result};
use headless_chrome::Tab;
use serde_json::Value;

use crate::{browser::goto, render::escape};

const DEFAULT_ACCENT: u32 = 0x1d7cf2;

fn api(tab: &Tab, path: &str) -> Result<Option<Value>> {
    api_as(tab, path, None)
}

fn api_as(tab: &Tab, path: &str, token: Option<&str>) -> Result<Option<Value>> {
    anyhow::ensure!(goto(tab, &format!("https://api.jailbreakchangelogs.com{path}"))?, "{path} didn't load");
    let read = match token {
        Some(t) => format!("fetch(location.href, {{ headers: {{ Authorization: {} }} }}).then(r => r.text())", serde_json::to_string(t)?),
        None => "document.body.innerText".into(),
    };
    let body = tab.evaluate(&read, token.is_some())?.value.context("empty API response")?;
    let v: Value = serde_json::from_str(body.as_str().unwrap_or_default()).with_context(|| format!("parsing {path}"))?;
    if !v.is_object() || v.get("detail").is_some() || v.get("error").is_some() {
        eprintln!("  no card for {path}: {v}");
        return Ok(None);
    }
    Ok(Some(v))
}

fn display_name(u: &Value) -> Option<&str> {
    let field = |k: &str| u[k].as_str().filter(|s| !s.is_empty() && *s != "None");
    field("global_name").or_else(|| field("username"))
}

pub fn theme_card(tab: &Tab, code: &str) -> Result<String> {
    let Some(t) = api(tab, &format!("/v2/themes/shared/{code}"))? else { return Ok(String::new()) };
    let var = |k: &str, fallback: &str| css_value(t["vars"][k].as_str().unwrap_or_default()).unwrap_or(fallback).to_string();
    let (bg, accent) = (var("--color-primary-bg", "#101014"), var("--color-highlight", "#1d7cf2"));
    let (button, button_text) = (var("--color-form-button", &accent), var("--color-form-button-text", "#fff"));

    let label = t["label"].as_str().unwrap_or(code);
    let author = match t["user_id"].as_str() {
        Some(id) => api(tab, &format!("/v2/users/{id}"))?.and_then(|u| display_name(&u).map(str::to_string)),
        None => None,
    };
    let icon = t["icon"].as_str().filter(|i| i.bytes().all(|b| b.is_ascii_alphanumeric() || b == b':' || b == b'-')).unwrap_or("lucide:palette");
    let icon_color = if accent.starts_with('#') { accent.replace('#', "%23") } else { "white".into() };
    let icon_url = format!("https://api.iconify.design/{}.svg?color={icon_color}", icon.replacen(':', "/", 1));

    Ok(format!(
        "<div class=\"card\" style=\"--underglow:{accent}\"><div class=\"avatar\" style=\"background:{bg};display:grid;place-items:center;outline:2px solid #ffffff14\">\
         <img src=\"{}\" style=\"width:110px;height:110px\"></div><div><div class=\"name\">{}</div><div class=\"handle\">{}</div>\
         <span class=\"pill\" style=\"background:{button};color:{button_text}\">Theme {}</span></div></div>",
        escape(&icon_url),
        escape(label),
        escape(&author.map(|a| format!("by {a}")).unwrap_or_default()),
        escape(code)
    ))
}

pub fn trade_card(tab: &Tab, code: &str) -> Result<String> {
    Ok(api(tab, &format!("/v2/calculator/share/{code}"))?.map(|t| trade_html(&t, "Trade Calculator")).unwrap_or_default())
}

pub fn trade_ad_card(tab: &Tab, id: &str) -> Result<String> {
    let Ok(token) = std::env::var("JBCL_TOKEN") else {
        eprintln!("  no JBCL_TOKEN, can't load trade ad {id}");
        return Ok(String::new());
    };
    let Some(t) = api_as(tab, &format!("/v2/trades/{id}"), Some(&token))? else { return Ok(String::new()) };
    Ok(trade_html(&t, &ad_footer(&t["user"])))
}

fn ad_footer(u: &Value) -> String {
    let field = |k: &str| u[k].as_str().filter(|s| !s.is_empty() && *s != "None");
    let name = field("roblox_username").or_else(|| display_name(u)).unwrap_or("Trade Ad");
    let avatar = field("roblox_avatar").or_else(|| field("avatar_url"));
    format!(
        "{}{}",
        avatar.map(|a| format!("<img src=\"{}\" onerror=\"this.remove()\">", escape(a))).unwrap_or_default(),
        escape(name)
    )
}

fn trade_html(t: &Value, footer: &str) -> String {
    let side = |key: &str, label: &str| {
        let items = t[key].as_array().map(Vec::as_slice).unwrap_or_default();
        let total: f64 = items.iter().map(|i| item_value(i) * i["amount"].as_f64().unwrap_or(1.0)).sum();
        let mut slots: String = items.iter().map(slot_html).collect();
        for _ in items.len()..items.len().max(1).div_ceil(8) * 8 {
            slots.push_str("<div class=\"slot\"><div class=\"orb\"></div></div>");
        }
        format!("<div class=\"side\"><div class=\"who\"><span>{label}</span><span>{}</span></div><div class=\"slots\">{slots}</div></div>", money(total))
    };
    format!(
        "<div class=\"trade\">{}{}<div class=\"bar\"><span>{footer}</span><img class=\"logo\" src=\"https://assets.jailbreakchangelogs.com/assets/logos/JBCL_Long_Transparent.webp\"></div></div>",
        side("offering", "Offering"),
        side("requesting", "Requesting")
    )
}

fn slot_html(i: &Value) -> String {
    let s = |k: &str| i[k].as_str().unwrap_or_default();
    let (kind, name) = (s("type"), s("name"));
    let mut badges = String::new();
    if let Some(n) = i["amount"].as_u64().filter(|&n| n > 1) {
        badges += &format!("<i>x{n}</i>");
    }
    if i["duped"].as_bool() == Some(true) {
        badges += "<i>Duped</i>";
    }
    if i["og"].as_bool() == Some(true) {
        badges += "<i>OG</i>";
    }
    format!(
        "<div class=\"slot\"><em>{}</em><b>{}</b><div class=\"orb{}\"><img src=\"{}\" onerror=\"this.remove()\"><span>{badges}</span></div><u>{}</u></div>",
        escape(kind),
        escape(&name.replace(" Level ", " Lvl ")),
        if kind.eq_ignore_ascii_case("vehicle") { " red" } else { " blue" },
        escape(&item_image(s("id"), kind, name)),
        money(item_value(i))
    )
}

fn item_image(id: &str, kind: &str, name: &str) -> String {
    const ITEMS: &str = "https://assets.jailbreakchangelogs.com/assets/images/items/480p";
    let enc = |s: &str| s.replace('%', "%25").replace(' ', "%20").replace('#', "%23").replace('?', "%3F");
    let kind = kind.to_lowercase();
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return format!("https://assets.jailbreakchangelogs.com/assets/items/trade_icons/{}.webp", enc(&id.to_lowercase().replace(' ', "_")));
    }
    match kind.as_str() {
        "horn" => "https://assets.jailbreakchangelogs.com/assets/audios/horns/thumbnails/480p/horn_thumbnail.webp".into(),
        "drift" => format!("{ITEMS}/drifts/thumbnails/{}.webp", enc(name)),
        "body color" | "furniture" | "hyperchrome" | "rim" | "spoiler" | "texture" | "tire sticker" | "tire style" | "vehicle" | "weapon skin" => {
            format!("{ITEMS}/{}s/{}.webp", enc(&kind), enc(name))
        }
        _ => format!("{ITEMS}/{}/{}.webp", enc(&kind), enc(name)),
    }
}

fn item_value(i: &Value) -> f64 {
    let duped = (i["duped"].as_bool() == Some(true)).then(|| parse_value(i["info"]["duped_value"].as_str()?)).flatten();
    duped.or_else(|| parse_value(i["info"]["cash_value"].as_str()?)).unwrap_or(0.0)
}

fn parse_value(s: &str) -> Option<f64> {
    let s = s.trim().to_ascii_lowercase().replace(',', "");
    let (num, mul) = match s.chars().last()? {
        'k' => (&s[..s.len() - 1], 1e3),
        'm' => (&s[..s.len() - 1], 1e6),
        'b' => (&s[..s.len() - 1], 1e9),
        _ => (s.as_str(), 1.0),
    };
    Some(num.trim().parse::<f64>().ok()? * mul)
}

fn money(v: f64) -> String {
    let digits = format!("{:.0}", v.max(0.0));
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    format!("${out}")
}

fn css_value(s: &str) -> Option<&str> {
    let ok = !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b" #(),.%-".contains(&b));
    ok.then_some(s)
}

pub fn profile_card(tab: &Tab, id: &str) -> Result<String> {
    let Some(u) = api(tab, &format!("/v2/users/{id}"))? else { return Ok(String::new()) };

    let field = |k: &str| u[k].as_str().filter(|s| !s.is_empty() && *s != "None").map(str::to_string);
    let color = |k: &str| u[k].as_u64().or_else(|| u[k].as_str()?.parse().ok()).map(|c| c as u32 & 0xffffff);
    let username = field("username").with_context(|| format!("no user {id}"))?;
    let name = display_name(&u).unwrap_or(&username).to_string();
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
    use super::{ad_footer, css_value, item_image, parse_value, money, text_color, trade_html};

    #[test]
    fn trade_values() {
        assert_eq!(parse_value("47m"), Some(47e6));
        assert_eq!(parse_value("1.5B"), Some(1.5e9));
        assert_eq!(parse_value("500k"), Some(500e3));
        assert_eq!(parse_value("N/A"), None);
        assert_eq!(money(47e6), "$47,000,000");
        assert_eq!(money(1.25e9), "$1,250,000,000");
        assert_eq!(money(999.0), "$999");
        assert_eq!(money(0.0), "$0");
        assert_eq!(
            item_image("556", "HyperChrome", "HyperDiamond Level 5"),
            "https://assets.jailbreakchangelogs.com/assets/images/items/480p/hyperchromes/HyperDiamond%20Level%205.webp"
        );
        let t = serde_json::json!({
            "offering": [{"id": "170", "duped": true, "amount": 2, "name": "Javelin", "type": "Vehicle", "info": {"cash_value": "47m", "duped_value": "43m"}}],
            "requesting": [{"id": "556", "name": "<b>", "type": "HyperChrome", "info": {"cash_value": "40m"}}]
        });
        let html = trade_html(&t, "Trade Calculator");
        assert!(html.contains("<span>Offering</span><span>$86,000,000</span>"));
        assert!(html.contains("<i>x2</i><i>Duped</i>"));
        assert!(html.contains("&lt;b&gt;") && !html.contains("<b><b>"));
        assert_eq!(html.matches("class=\"slot\"").count(), 16);
        let user = serde_json::json!({"roblox_username": "KingJ67901", "roblox_avatar": "https://x/a\"b", "global_name": "MyKel"});
        assert_eq!(
            ad_footer(&user),
            "<img src=\"https://x/a&quot;b\" onerror=\"this.remove()\">KingJ67901"
        );
        assert!(ad_footer(&serde_json::json!({"global_name": "MyKel"})) == "MyKel");
    }

    #[test]
    fn only_plain_css_colours() {
        assert_eq!(css_value("#ff7a1a"), Some("#ff7a1a"));
        assert_eq!(css_value("hsl(260, 3%, 18%)"), Some("hsl(260, 3%, 18%)"));
        assert_eq!(css_value("red;background:url(x)"), None);
        assert_eq!(css_value("\"><script>"), None);
        assert_eq!(css_value(""), None);
    }

    #[test]
    fn picks_readable_text() {
        assert_eq!(text_color(&[0x1d7cf2]), "#fff");
        assert_eq!(text_color(&[0xa8b2b9]), "#0b0b0f");
        assert_eq!(text_color(&[0x101014]), "#fff");
        assert_eq!(text_color(&[0xffff00]), "#0b0b0f");
        assert_eq!(text_color(&[0xa8b2b9, 0x6f79bf]), "#0b0b0f");
    }
}
