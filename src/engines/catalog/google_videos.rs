// Google Videos, transcribed from upstream `searx/engines/google_videos.py`
// (commit 3cd69d30e2a78dfc817be9e349e7c2e4317c92e3) together with the shared
// request builder in `searx/engines/google.py`. The upstream `settings.yml`
// block carries no host/parameter overrides (only `shortcut: gov`), so the
// endpoint below is upstream's real one.
//
// Upstream request: `https://www.google.com/wml/search?q=<query>&sca_esv=1&tbm=vid`
// plus the shared `ie`/`oe` encoding parameters, i.e. the legacy WML/XML
// layout (`about["results"] == "XML"`), not the JS-driven web UI. The response
// body is XHTML, so it is scraped here with the same XPath-to-CSS translation
// upstream uses: `div.zMzFAb` items, `a.fuLhoc` link, `span.CVA68e` title.
//
// DISABLED. Upstream does not reach this layout with an honest client: it sets
// a random Nokia feature-phone `User-Agent` *and* `params["impersonate"] =
// "chrome99_android"` (TLS/browser fingerprint spoofing) to be served the WML
// markup, and the very reason `detect_google_sorry()` exists is that Google
// answers this endpoint with `sorry.google.com` / `/sorry/index` CAPTCHA
// interstitials. The html_scrape adapter sends one plain, honest GET with the
// shared user agent, so this engine is not reachable without fingerprint
// spoofing and bot-detection evasion, which is outside the safety boundary.
// Enable only if you are willing to accept Google blocking the client.
//
// Omitted because the adapters cannot express them:
//   - the per-request Nokia `User-Agent` / `chrome99_android` TLS impersonation
//     and the `CONSENT=YES+` cookie, i.e. the reason this engine is disabled;
//   - `unwrap_google_url()`: upstream strips Google's `/url?q=...&sa=U`
//     redirector, the adapter has no such hook, so links stay as Google
//     redirect URLs;
//   - the video-only metadata: `thumbnail` from `img.SygO9d` (and the
//     `default.jpg` -> `hqdefault.jpg` swap) and `length` parsed out of
//     `span.YVIcad`; upstream emits `Video` results, here they normalize to
//     title + link only (there is no snippet in the upstream video markup, so
//     `snippet_selector` is omitted rather than invented);
//   - pagination: upstream pages with `start=(pageno-1)*10` up to `max_page=50`
//     and the adapter has no pagination;
//   - the dynamic `hl` (interface language), `lr`/`cr` locale restrictions that
//     upstream drops via `use_locales=False`, the `safe=safesearch` filter and
//     the `tbs=qdr:<d|w|m|y>` time range; `hl` is derived from the fetched
//     Google locale traits and cannot be a static `param_` value, the rest are
//     not sent by this adapter.
pub fn definition() -> crate::engines::catalog::CatalogEntry {
    crate::engine_catalog_entry!("google_videos", "html_scrape", enabled = false, [
        "endpoint" => "https://www.google.com/wml/search",
        "query_param" => "q",
        "param_tbm" => "vid",
        "param_sca_esv" => "1",
        "param_ie" => "utf8",
        "param_oe" => "utf8",
        "result_selector" => "div.zMzFAb",
        "link_selector" => "a.fuLhoc",
        "title_selector" => "span.CVA68e",
    ])
}
