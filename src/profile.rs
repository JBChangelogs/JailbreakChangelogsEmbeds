use anyhow::{Context, Result};
use headless_chrome::Tab;

use crate::{browser::goto, render::escape};

pub fn profile_card(tab: &Tab, id: &str) -> Result<String> {
    anyhow::ensure!(goto(tab, &format!("https://api.jailbreakchangelogs.com/v2/users/{id}"))?, "user API didn't load");
    let body = tab.evaluate("document.body.innerText", false)?.value.context("empty API response")?;
    let u: serde_json::Value = serde_json::from_str(body.as_str().unwrap_or_default()).context("parsing user JSON")?;

    let field = |k: &str| u[k].as_str().filter(|s| !s.is_empty() && *s != "None").map(str::to_string);
    let username = field("username").with_context(|| format!("no user {id}"))?;
    let name = field("global_name").unwrap_or_else(|| username.clone());
    let avatar = field("custom_avatar")
        .filter(|_| u["settings_v2"]["custom_avatar"].as_bool() == Some(true))
        .or_else(|| field("avatar").map(|a| format!("https://cdn.discordapp.com/avatars/{id}/{a}.png?size=512")))
        .unwrap_or_else(|| "https://cdn.discordapp.com/embed/avatars/0.png".into());
    let number = u["usernumber"].as_u64().map(|n| format!("<span class=\"pill\">User #{n}</span>")).unwrap_or_default();

    Ok(format!(
        "<div class=\"card\"><img class=\"avatar\" src=\"{}\"><div><div class=\"name\">{}</div><div class=\"handle\">@{}</div>{number}</div></div>",
        escape(&avatar),
        escape(&name),
        escape(&username)
    ))
}
