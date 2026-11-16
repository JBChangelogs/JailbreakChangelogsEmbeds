<p align="center">
  <img src="https://assets.jailbreakchangelogs.com/assets/logos/JBCL_Long_Transparent.png" alt="Jailbreak Changelogs" width="520">
</p>

<p align="center">
  Generates the 3D-tilted embed images used for link previews on Jailbreak Changelogs.
</p>

<p align="center">
  <a href="https://jailbreakchangelogs.com"><strong>Website</strong></a> •
  <a href="https://embeds.jailbreakchangelogs.com/og.png">Example</a>
</p>

---

> [!NOTE]
> This is a fan-made project operated by Jailbreak Changelogs LLC and is not affiliated with or endorsed by Badimo — the development team behind Roblox Jailbreak.

---

## How it works

embedgen opens a page in headless Chrome, hides banners, ads and popups, screenshots it, and places the screenshot tilted in 3D on a branded template with a title. Profile pages also get a card with the user's avatar, name, handle and user number.

## Usage

Requires Rust and Chrome or Chromium.

```sh
cargo run --release                   # pregenerate every embed in embeds.json into out_dir
cargo run --release -- serve          # HTTP server on $PORT (default 8080)
cargo run --release -- video          # crossfade the pregenerated images into an mp4 (needs ffmpeg)
```

Each command takes an optional config path, e.g. `cargo run --release -- serve path/to/embeds.json`.

### Server endpoints

| Endpoint | Description |
|---|---|
| `GET /og/<route>.png` | Embed for any page, e.g. `/og/values.png`, `/og/users/<id>.png`, `/og.png` for the home page. Public and cacheable. |
| `GET /<file>` | A pregenerated image from `out_dir`. |

## Configuration

`embeds.json` is not committed. A minimal one:

```json
{
  "base_url": "https://jailbreakchangelogs.com",
  "site_name": "Jailbreak Changelogs",
  "out_dir": "out",
  "delay_ms": 3000,
  "concurrency": 4,
  "dismiss_buttons": ["Got it"],
  "hide": ["[id^=\"np-\"]"],
  "embeds": [
    { "route": "/values", "file": "values.png", "title": "Item values for Roblox Jailbreak", "subtitle": "Trusted & used by thousands" }
  ]
}
```

| Field | Description |
|---|---|
| `base_url` | Site the routes are loaded from. |
| `site_name` | Stripped from page titles, so `Values - Jailbreak Changelogs` becomes `Values`. |
| `out_dir` | Where pregenerated images are written. |
| `delay_ms` | Longest wait for fade-in animations after a page loads. |
| `concurrency` | Browsers running at once. |
| `dismiss_buttons` | Buttons clicked before screenshotting, matched by text or aria-label. |
| `hide` | CSS selectors hidden before screenshotting. |
| `loading` | CSS selectors for loading placeholders; screenshots wait until none are on screen. |
| `embeds` | Pages to pregenerate. `title` defaults to the page's own title. Optional: `user` (Discord ID for a profile card), `delay_ms`, `css`, `transparent`. |

## Docker

```sh
docker build -t embedgen .
docker run -p 8080:8080 -v ./embeds.json:/app/embeds.json:ro -v ./out:/app/out:ro embedgen
```