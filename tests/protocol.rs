//! Protocol tests against vectors recorded from the original client replay
//! (investigation/evidence/native-fixture-runtime.txt; fixture key, not a real secret).
use re0drive::models::{Resource, Subscription};
use re0drive::providers::{canonical, normalize_resources, parse_115_share, points, sign};
use serde_json::json;
use url::Url;

const KEY: &str = "fixture-learning-key";
const INST: &str = "inst_fixture_learning";
const TS: &str = "1780000000";
const NONCE: &str = "fixture_nonce";

fn sig(method: &str, url: &str, body: &[u8]) -> String {
    sign(
        KEY,
        &canonical(method, &Url::parse(url).unwrap(), INST, TS, NONCE, body),
    )
}

#[test]
fn checkin_canonical_matches_fixture() {
    let c = canonical(
        "POST",
        &Url::parse("https://re0.tgtodrive.top/api/checkin").unwrap(),
        INST,
        TS,
        NONCE,
        br#"{"is_gambler":false}"#,
    );
    assert_eq!(
        c,
        "POST\n/api/checkin\n\ninst_fixture_learning\n1780000000\nfixture_nonce\n671664eefd9533eb35fc10578461d349dd0b174c5bbcf3a2fe947319488ac51e"
    );
}

#[test]
fn signatures_match_original_client() {
    assert_eq!(
        sig("GET", "https://re0.tgtodrive.top/api/ping", b""),
        "2e114dfb37f7c888087089fdf4d2762a6ec30578aad6e9b645d2f16b28e23232"
    );
    assert_eq!(
        sig("GET", "https://re0.tgtodrive.top/api/token/status", b""),
        "263db9ed08199c029b99cfa9c0331d27c9b298be9463d47f28c1e5f70ca7631b"
    );
    assert_eq!(
        sig(
            "POST",
            "https://re0.tgtodrive.top/api/resources/unlock",
            br#"{"slug":"fixture-slug"}"#
        ),
        "f1d67fb2f8337d9d3f9fae9d52aed817feddd099ec73bcbb7f22f02292c5678a"
    );
    assert_eq!(
        sig(
            "POST",
            "https://re0.tgtodrive.top/api/checkin",
            br#"{"is_gambler":false}"#
        ),
        "93de167eaaa76cafb3618b60e872c00cde4ab52717a0eea85f80fd2546035879"
    );
    assert_eq!(
        sig(
            "POST",
            "https://re0.tgtodrive.top/api/checkin",
            br#"{"is_gambler":true}"#
        ),
        "b0585d6d92e9123ba4dfc7ba3d7faa099935adb642f1d5b7ade8c9ecb43a65a3"
    );
    // serde_json must produce the same compact bytes the original client signs
    assert_eq!(
        serde_json::to_vec(&json!({"slug":"fixture-slug"})).unwrap(),
        br#"{"slug":"fixture-slug"}"#
    );
}

#[test]
fn auth_url_signature_matches_fixture() {
    let url = "https://re0.tgtodrive.top/auth/start?install_id=inst_fixture_learning&ts=1780000000&nonce=fixture_nonce";
    assert_eq!(
        sig("GET", url, b""),
        "30c10c154c8843756fbb656a1eb8c750a9fc7b820c01015c264261cdfd083af9"
    );
}

#[test]
fn query_canonicalization_matches_fixture() {
    let c = canonical(
        "GET",
        &Url::parse("https://x/api/fixture?z=2&a=hello%20world&sig=zzz").unwrap(),
        INST,
        TS,
        NONCE,
        b"",
    );
    assert_eq!(c.lines().nth(2).unwrap(), "a=hello+world&z=2");
    let c = canonical(
        "GET",
        &Url::parse("https://x/p?z=2&z=1&a=hello+world&a=").unwrap(),
        INST,
        TS,
        NONCE,
        b"",
    );
    assert_eq!(c.lines().nth(2).unwrap(), "a=&a=hello+world&z=1&z=2");
}

#[test]
fn parses_115_share_links() {
    assert_eq!(
        parse_115_share("https://115.com/s/sw3abc?password=x1y2").unwrap(),
        ("sw3abc".into(), "x1y2".into())
    );
    assert_eq!(
        parse_115_share("https://115cdn.com/s/abc-1?pwd=zz#").unwrap(),
        ("abc-1".into(), "zz".into())
    );
    assert!(parse_115_share("https://evil.example/s/abc?password=1").is_err());
    assert!(parse_115_share("https://115.com.evil.example/s/abc").is_err());
    assert!(parse_115_share("https://115.com/x/abc").is_err());
}

#[test]
fn normalizes_resources_and_points() {
    let v = json!({"success":true,"data":{"115":[{"slug":"a1","title":"Movie 2160p","unlock_points":"3","quality":"2160p"}],
        "quark":[{"slug":"q1","title":"kept but not selectable"}],"other":[{"slug":"b2","pan_type":"115","title":"Movie 1080p","is_unlocked":true}]}});
    let mut r = normalize_resources(&v);
    r.sort_by(|a, b| a.slug.cmp(&b.slug));
    assert_eq!(r.len(), 3);
    assert_eq!((r[0].slug.as_str(), r[0].unlock_points), ("a1", Some(3)));
    assert!(r[0].selectable);
    assert!(r[1].is_unlocked && r[1].selectable);
    assert_eq!((r[2].slug.as_str(), r[2].provider.as_str(), r[2].pan_type.as_str(), r[2].selectable), ("q1", "quark", "quark", false));
    assert_eq!(
        points(&json!({"actual_unlock_points":5,"unlock_points":2})),
        Some(5)
    );
    assert_eq!(points(&json!({})), None);
}

fn res(title: &str, points: Option<i64>, unlocked: bool) -> Resource {
    Resource {
        slug: "s".into(),
        title: title.into(),
        provider: "115".into(),
        source_url: String::new(),
        unlock_points: points,
        is_unlocked: unlocked,
        quality: String::new(),
        size: String::new(),
        tmdb_id: String::new(),
        media_type: "movie".into(),
        ..Default::default()
    }
}

#[test]
fn subscription_filters() {
    let rule = Subscription {
        max_points: 4,
        quality: "2160p".into(),
        include_keywords: "remux".into(),
        exclude_keywords: "枪版, TC".into(),
        ..Default::default()
    };
    assert!(rule.matches(&res("Dune 2160p REMUX", Some(2), false)));
    assert!(
        !rule.matches(&res("Dune 2160p REMUX", Some(9), false)),
        "over cap"
    );
    assert!(
        !rule.matches(&res("Dune 2160p REMUX", None, false)),
        "unknown cost"
    );
    assert!(
        rule.matches(&res("Dune 2160p REMUX", None, true)),
        "already owned"
    );
    assert!(
        !rule.matches(&res("Dune 1080p REMUX", Some(0), false)),
        "quality"
    );
    assert!(
        !rule.matches(&res("Dune 2160p REMUX tc", Some(0), false)),
        "excluded"
    );
}
