use crate::auth::*;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde_json::{json, Value};
const TEST_RSA_PRIVATE: &str = "-----BEGIN PRIVATE KEY-----
MIIEvAIBADANBgkqhkiG9w0BAQEFAASCBKYwggSiAgEAAoIBAQDWpMw1Mzy5hWFW
PWfZnF6hCB3pm/nHbFJZ820Qf3a9eSWaRJXJiuj1r5txBcvBkbc8xkzXCimUBhq1
zE0+xF+y0THmcY4aGHjRuJoDUdGMoayPkRVGgo4X5S8j7Jer2C41XXMuzSYrHVgv
5+5bczmTg8N33FbLVCmFUtTswALBw2dCCIK61IVjW6LV3QqembS6yPt4aPynvkSk
WrfAyirjdLSG6FcmpxcGSgeTJVPk16N9ab/tGBEQ+CYs+/5WIXVjW+xtEeb/Uj4l
WY//YTwM8kLG9C77oAg5x41SrS+p3xEtHngbds2lE6UVqVmCJsavvMnRBXdYyHxP
B9xMx4phAgMBAAECggEAFwIkGjVPdncV3aXYMJ9WlPYu56xmuGH0C2ygxa6WKr4W
YeylbkznxzNPayyDAJFHRjpfAQOXvKRxX0jCxH5OIFfUnKZSRFmYdOmwB7hQ6T1o
5xxXHp9uziCs/pG1WfBC1slZSBxpUaCUBBGdbzvhIX8TuFDcFHBlgYNFLAoymXSv
poJrPrgsEWd/JeYDezfYhh6KF4CKt1zKgrIZ4sfAng57VDH0Bt9gLjR5r+n+6xxL
+WoERppAPJGqyFShbVxz8LZOExhe9DWOelIQUO2hMnzBtMDv3dAJBvjtbICJrFtz
/BUEIYt8wDv4yVXz1O0u789vyMa/xwulxHhUuV/d2QKBgQDsNhAaAJksFNvhfkWp
IeOdYa5pSyfYE1+UF8dblvdIJOngfFhU/KasIh0B3o5YxEQFwWCE5IsIN7jT62nN
CBmtnQnL+XjMQCaDHZvlyDpwmvlyuPEZzTaxGe2IZ2usCUu1T3r4pLtVE7VIldle
HyOpa044dmNl3AxJPvVbQrzWyQKBgQDooC9/gJvJodF1MbSVT5qlqxDJwZ9HgJY/
fhdJKfW+sqQMpApKYvrolMsbDfeo1fviJOTxRfF8BpsUZfN6zSKYIInz1WlIkWRT
P0sJMZUo4kLHt/PIS88aIbK2cSwNqLl/7n4PoTxqG0P368goy9QbOOmD2X2mA1l+
P8rReXSq2QKBgB8IU0E3RuhVrTWIw1ofC6pHhQRsTUXD9dCc9yH/SWl/AALwEyLH
NpZyvODb/lOHJXCkISwUYnen6m5dBT9cixMWCI11rvsWini7URn1Hkhg89iwl2xO
W5sUzvIWtDyb1Ahz8rHr4nig6DYrCa2l5aeCY3pjg1eEe1C8JrvgnrKRAoGAWJfA
3x8ctZKiEa7nZkHV1KgskZnizjljfzTHK38GbyTbo1DJ9oBxrCgWnewY2Lz926dP
Za/Mgv6FCyS0sJz1QtiJkUpCeXedrLKbIho3A0YARs2A01RDwGD7Dc5WB7GtS9KJ
QeyW9JYDsaSjKx5NXjyzehpXZuU5rQIgfNxzmSkCgYAwJzAzQEg8FbOg459Hh6rn
pnuHt9a8RF8htS1xMi00e2AiCC7vnXVpvxONlQmrDp+M5wWUE/XLaiSVjCZ54+uy
9XaMUof1SbKARdvaZ3PTBC1IyPD4hs2XIHjTFIgmctvTaaygDiBf/QQj/235lOXy
+8oUanXXTlY3JRInuS+JgA==
-----END PRIVATE KEY-----";
pub(crate) const TEST_RSA_PUBLIC: &str = "-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA1qTMNTM8uYVhVj1n2Zxe
oQgd6Zv5x2xSWfNtEH92vXklmkSVyYro9a+bcQXLwZG3PMZM1woplAYatcxNPsRf
stEx5nGOGhh40biaA1HRjKGsj5EVRoKOF+UvI+yXq9guNV1zLs0mKx1YL+fuW3M5
k4PDd9xWy1QphVLU7MACwcNnQgiCutSFY1ui1d0Knpm0usj7eGj8p75EpFq3wMoq
43S0huhXJqcXBkoHkyVT5NejfWm/7RgREPgmLPv+ViF1Y1vsbRHm/1I+JVmP/2E8
DPJCxvQu+6AIOceNUq0vqd8RLR54G3bNpROlFalZgibGr7zJ0QV3WMh8TwfcTMeK
YQIDAQAB
-----END PUBLIC KEY-----";
pub(crate) fn sign(claims: &Value) -> String {
    let mut h = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    h.kid = Some("test-kid".into());
    jsonwebtoken::encode(
        &h,
        claims,
        &jsonwebtoken::EncodingKey::from_rsa_pem(TEST_RSA_PRIVATE.as_bytes()).unwrap(),
    )
    .unwrap()
}
#[tokio::test]
async fn browser_originator_verify_exposes_verified_stable_identity() {
    let mut state = crate::AppState::test_without_runtime();
    state.auth.config = std::sync::Arc::new(AuthConfig {
        oidc_base: Some("https://idp.test".into()),
        ..AuthConfig::default()
    });
    state.auth.keys.preload(
        "test-kid",
        jsonwebtoken::DecodingKey::from_rsa_pem(TEST_RSA_PUBLIC.as_bytes()).unwrap(),
    );
    let token = sign(
        &json!({"iss":"https://idp.test/application/o/browser/", "sub":"human-one", "azp":"shared-browser-client", "preferred_username":"alice", "exp":now_epoch()+600}),
    );
    let mut h = HeaderMap::new();
    h.insert("authorization", format!("Bearer {token}").parse().unwrap());
    let response = verify(State(state), h).await.into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 8192)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["caller"], "alice");
    assert_eq!(value["issuer"], "https://idp.test/application/o/browser/");
    assert_eq!(value["subject"], "human-one");
    assert_eq!(value["principal_kind"], "oidc");
}
