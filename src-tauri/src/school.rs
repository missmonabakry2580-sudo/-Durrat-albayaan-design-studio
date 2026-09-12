//! وصلةُ منصة المدرسة — يدُ أمين داخل بوابة دُرة البيان.
//!
//! ═══ ما تفعله، بنصّ منى ═══
//! «من ضمن مهامه أنه يشتغل في كل المهام في المنصة، بس يبلّغني المهمة
//! الموجودة وبعطيه قرار ينفّذ — وينفّذ على طول.» فهذا الملفّ طرفان:
//! `pending()` يجمع ما ينتظرها فيُبلِّغها، و`execute()` ينفّذ إجراءً واحدًا
//! **بعد كلمتها** لا قبلها.
//!
//! ═══ ولماذا هذا الملفّ في Rust لا في الواجهة ═══
//! `connect-src 'self'` يمنع واجهة أمين من الشبكة أصلًا — وهو قيدٌ على
//! **واجهته**، لا منعٌ له من العمل. فكل نداءٍ خارجيّ يخرج من هنا، حيث تسكن
//! السياسة (`policy.rs`) والسجلّ غير القابل للتعديل (`audit.rs`). وهذا
//! بالضبط ما يجعل عمله في المنصة **مسموحًا وموثَّقًا** في الوقت نفسه.
//!
//! ═══ ولماذا لا تأخذ هذه الدوالُّ `&Connection` ═══
//! لأن `MutexGuard` ليس `Send`: قفلٌ مُمسَك عبر `.await` **يمنع التصميف**
//! أصلًا (وهو الدرس المكتوب في `tools.rs` عن نفس المشكلة). فالبيانات
//! تُقرأ من القاعدة **قبل** الشبكة في `load_config`، ويُعاد الرمز الجديد
//! **بعدها** في `TokenUpdate` ليُخزَّنه المُنادي بقفلٍ ثانٍ قصير. فلا قفل
//! مفتوحٌ أثناء نداءٍ شبكيّ أبدًا.
//!
//! ═══ وهذا ليس أول وصلٍ بالمنصة — بل الخطوة المكتوبة بعده ═══
//! في `mobile/school-admin.js` وصلٌ قائم ومُختبَر (٢٩ أغسطس) لبوابة
//! **حسابات** الإدارة: سبع أدوات `school_*`، بنفس مسار الدخول وبوّابة
//! تأكيدٍ لكل كتابة. وخُطّة `docs/ARCHITECTURE.md` هناك تقول حرفيًّا:
//! «extend to the other portals (finance/academic/admissions) the same
//! way, each write gated» — وهذا هو. فالطرف هناك في تطبيق الهاتف (PWA)
//! ويعمل على **الحسابات**، وهذا الطرف في نواة سطح المكتب ويعمل على
//! **المهامّ المعلَّقة** (اعتماد · تذكير)، حيث الموجزُ الصباحي والسجلّ.
//!
//! **وما لم يُبنَ بعد يُقال:** لا نظير لهاتين الأداتين في `mobile/` بعد،
//! فأمين على الهاتف لا يرى المعلَّق اليوم. وهو عملٌ معلَن لا مُخفى.
//!
//! ═══ والهُويّة ═══
//! أمين يسجّل الدخول **بحسابٍ إداريّ في المنصة** (بريد + كلمة سرّ) عبر
//! Firebase REST، فيحصل على رمزٍ صلاحيته ساعة، ويحمله إلى `‎/api/amin`.
//! فلا صلاحيات أوسع من صلاحية ذلك الحساب، ولا مفتاحٌ ثابت في الشيفرة —
//! وهو نفس نموذج `mobile/school-admin.js` بعينه، لا نموذجٌ ثانٍ.
//!
//! **وتُخزَّن كلمة السرّ في جدول `settings` المحليّ** — على القرص، لا في
//! Keychain. وهذا ليس استحسانًا بل **القرار القائم الموثَّق في
//! `docs/SECURITY.md`** بعد فشل `keyring` على ماك حقيقيّ (`set_secret`
//! ينجح و`get_secret` يرجع `NoEntry` في الجلسة نفسها). ولذلك: **الأفضل أن
//! يكون لأمين حسابٌ إداريّ خاصّ به** لا حساب المديرة نفسه — فسحبُه لا يمسّ
//! دخولها هي.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::commands::{get_setting, set_setting};

/// عنوان المنصة — **النطاق الرسميّ للمدرسة**، قابلٌ للتغيير من الإعدادات
/// دون بناءٍ جديد.
///
/// ⚠️ **ولا يُستبدل بعنوان `lovable.app`**: ذاك النطاق كلُّه يُعيد التوجيه
/// (302) إلى هذا. و`reqwest` يتبع التوجيه لكنه **يُسقط ترويسة
/// `Authorization` عند تغيُّر المضيف** — فالنتيجة ٤٠١ دائمًا، أي «أمين لا
/// يرى شيئًا» بلا سببٍ ظاهر. رُصد هذا في فحصٍ حيّ لا في مراجعة.
const DEFAULT_BASE_URL: &str = "https://portal.durratalbayaan.edu.om";

/// **مفتاح الويب العامّ للمشروع — وليس سرًّا.** هو نفسه مشحونٌ في حزمة كل
/// صفحةٍ يفتحها أي زائر للموقع، ووظيفته تعريفُ المشروع لا إثباتُ الهوية:
/// من يملكه لا يملك شيئًا، لأن الصلاحية تُحسم بتسجيل الدخول وقواعد
/// Firestore. وُضع هنا لتكون الحقول التي تُدخلها منى **اثنين لا ثلاثة**.
const FIREBASE_WEB_API_KEY: &str = "AIzaSyDxRZhnNber1MMqCe_Y-nEbr4GI0lzTE6g";

pub(crate) const SCHOOL_EMAIL_KEY: &str = "school_portal_email";
pub(crate) const SCHOOL_PASSWORD_KEY: &str = "school_portal_password";
pub(crate) const SCHOOL_BASE_URL_KEY: &str = "school_portal_base_url";
const SCHOOL_TOKEN_KEY: &str = "school_portal_id_token";
const SCHOOL_TOKEN_EXPIRY_KEY: &str = "school_portal_id_token_expires_at";

/// هامشُ أمانٍ قبل انتهاء الرمز. رمزٌ ينتهي **أثناء** الطلب نفسه يُقرأ عند
/// منى «فشل» لا «انتهت الجلسة» — فيُعاد تسجيل الدخول قبل ذلك بدقيقتين.
const TOKEN_SAFETY_MARGIN_SECS: i64 = 120;

// ---------------------------------------------------------------------------
// شكلُ ما يُقرأ من `/api/amin`
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingAction {
    pub kind: String,
    /// ما تقرأه منى قبل أن تقول «نفّذ» — جاهزٌ بالعربية من المنصة.
    pub label: String,
    #[serde(default, rename = "rowIndexHint")]
    pub row_index_hint: i64,
    /// بصمةُ الصفّ. **تُمرَّر كما وصلت** ولا تُبنى هنا: المنصة تُعيد إيجاد
    /// الصفّ بها على قراءةٍ جديدة، فلا يُنفَّذ على صفٍّ أزاحه غيره.
    #[serde(default)]
    pub r#match: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingItem {
    pub id: String,
    /// ٠ = أسرةٌ تنتظر · ١ = مال · ٢ = ورق. والأصغر أولًا.
    pub tier: i64,
    pub title: String,
    pub detail: String,
    pub count: i64,
    /// الشاشة التي تُغلق البند — يقولها أمين لها حين لا إجراء له.
    pub to: String,
    #[serde(default)]
    pub names: Vec<String>,
    #[serde(default)]
    pub actions: Vec<PendingAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pending {
    #[serde(rename = "generatedAt")]
    pub generated_at: String,
    #[serde(rename = "familyWaiting")]
    pub family_waiting: i64,
    pub items: Vec<PendingItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteOutcome {
    pub ok: bool,
    /// `done` نُفِّذ · `already` لم يكن معلَّقًا فلم يُغيَّر شيء · `gone` لم يُوجَد.
    pub status: String,
    /// جملةٌ عربية تُقال لمنى كما هي.
    pub what: String,
}

// ---------------------------------------------------------------------------
// الإعداد والهُويّة — ما يُقرأ من القاعدة قبل الشبكة، وما يُكتب بعدها
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SchoolConfig {
    pub base_url: String,
    email: String,
    password: String,
    token: Option<String>,
    token_expires_at: i64,
}

/// رمزٌ جديد يستحقّ الحفظ. يُعاد للمُنادي بدل أن يُكتب هنا — لأن الكتابة
/// تحتاج قفل القاعدة، والقفل لا يُمسَك عبر الشبكة.
#[derive(Debug, Clone)]
pub struct TokenUpdate {
    pub token: String,
    pub expires_at: i64,
}

pub fn has_credentials(conn: &Connection) -> bool {
    let filled = |k: &str| {
        get_setting(conn, k)
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false)
    };
    filled(SCHOOL_EMAIL_KEY) && filled(SCHOOL_PASSWORD_KEY)
}

pub fn base_url(conn: &Connection) -> String {
    get_setting(conn, SCHOOL_BASE_URL_KEY)
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

/// يُقرأ كل ما تحتاجه الشبكة **مرّةً واحدة**، ثم يُفتح القفل.
pub fn load_config(conn: &Connection) -> Result<SchoolConfig, String> {
    let email = get_setting(conn, SCHOOL_EMAIL_KEY).unwrap_or_default();
    let password = get_setting(conn, SCHOOL_PASSWORD_KEY).unwrap_or_default();
    if email.trim().is_empty() || password.trim().is_empty() {
        return Err("لم تُدخل بيانات حساب المنصة بعد — أضِفها من إعدادات أمين.".to_string());
    }
    Ok(SchoolConfig {
        base_url: base_url(conn),
        email: email.trim().to_string(),
        password,
        token: get_setting(conn, SCHOOL_TOKEN_KEY).filter(|t| !t.trim().is_empty()),
        token_expires_at: get_setting(conn, SCHOOL_TOKEN_EXPIRY_KEY)
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0),
    })
}

pub fn store_token(conn: &Connection, update: &TokenUpdate) -> Result<(), String> {
    set_setting(conn, SCHOOL_TOKEN_KEY, &update.token)?;
    set_setting(
        conn,
        SCHOOL_TOKEN_EXPIRY_KEY,
        &update.expires_at.to_string(),
    )
}

/// يُنسى الرمز المخزَّن (خروج، أو تغيير بيانات).
pub fn forget_token(conn: &Connection) {
    let _ = conn.execute(
        "DELETE FROM settings WHERE key IN (?1, ?2)",
        [SCHOOL_TOKEN_KEY, SCHOOL_TOKEN_EXPIRY_KEY],
    );
}

/// تسجيلُ دخولٍ جديد. يُنادى فقط حين لا رمزَ صالحًا.
async fn sign_in(cfg: &SchoolConfig) -> Result<TokenUpdate, String> {
    let url = format!(
        "https://identitytoolkit.googleapis.com/v1/accounts:signInWithPassword?key={FIREBASE_WEB_API_KEY}"
    );
    let res = reqwest::Client::new()
        .post(&url)
        .json(&json!({
            "email": cfg.email,
            "password": cfg.password,
            "returnSecureToken": true,
        }))
        .send()
        .await
        .map_err(|e| format!("تعذّر الوصول إلى المنصة: {e}"))?;
    let status = res.status();
    let body: Value = res
        .json()
        .await
        .map_err(|e| format!("ردٌّ غير مفهوم من المنصة: {e}"))?;
    if !status.is_success() {
        // رسالةُ Google تقنيّة («INVALID_LOGIN_CREDENTIALS») — تُترجم لأن
        // منى هي من يقرؤها، وهي التي تُصلح السبب.
        let code = body
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("");
        return Err(match code {
            "INVALID_LOGIN_CREDENTIALS" | "INVALID_PASSWORD" | "EMAIL_NOT_FOUND" => {
                "بيانات حساب المنصة غير صحيحة — راجعي البريد وكلمة السرّ في إعدادات أمين."
                    .to_string()
            }
            "USER_DISABLED" => "حساب المنصة موقوف.".to_string(),
            other => format!("تعذّر تسجيل الدخول إلى المنصة ({other})."),
        });
    }
    let token = body
        .get("idToken")
        .and_then(|t| t.as_str())
        .ok_or_else(|| "المنصة لم تُرجع رمز هوية.".to_string())?
        .to_string();
    let expires_in = body
        .get("expiresIn")
        .and_then(|v| v.as_str())
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(3600);
    Ok(TokenUpdate {
        token,
        expires_at: chrono::Utc::now().timestamp() + expires_in,
    })
}

/// الرمزُ المخزَّن إن كان فيه بقيّةُ صلاحية، وإلا فتسجيلُ دخول.
async fn usable_token(cfg: &SchoolConfig) -> Result<(String, Option<TokenUpdate>), String> {
    let now = chrono::Utc::now().timestamp();
    if let Some(token) = cfg.token.as_ref() {
        if cfg.token_expires_at - now > TOKEN_SAFETY_MARGIN_SECS {
            return Ok((token.clone(), None));
        }
    }
    let fresh = sign_in(cfg).await?;
    Ok((fresh.token.clone(), Some(fresh)))
}

/// رسالةُ الخطأ التي تُعيدها المنصة، لا «فشل الطلب» — منى تقرؤها وتُصلح بها.
fn platform_error(status: reqwest::StatusCode, body: &Value) -> String {
    let msg = body
        .get("error")
        .and_then(|e| e.as_str())
        .unwrap_or("")
        .trim();
    if !msg.is_empty() {
        return msg.to_string();
    }
    match status.as_u16() {
        401 => "انتهت جلسة أمين في المنصة — سيسجّل الدخول من جديد.".to_string(),
        403 => "حساب أمين في المنصة ليس حساب إدارة.".to_string(),
        s => format!("المنصة ردّت بالرمز {s}."),
    }
}

/// طلبٌ واحد إلى `‎/api/amin`، ومعه **محاولةٌ ثانية واحدة** بعد ٤٠١ برمزٍ
/// جديد: رمزٌ انتهى بين طلبين ليس خطأً تُبلَّغ به منى، بل شيءٌ يُصلحه أمين.
async fn request(
    cfg: &SchoolConfig,
    body: Option<&Value>,
) -> Result<(Value, Option<TokenUpdate>), String> {
    let url = format!("{}/api/amin", cfg.base_url);
    let mut carried: Option<TokenUpdate> = None;
    for attempt in 0..2 {
        // في المحاولة الثانية يُهمَل الرمز المخزَّن ويُسجَّل الدخول حتمًا.
        let (token, fresh) = if attempt == 0 {
            usable_token(cfg).await?
        } else {
            let t = sign_in(cfg).await?;
            (t.token.clone(), Some(t))
        };
        if fresh.is_some() {
            carried = fresh;
        }
        let client = reqwest::Client::new();
        let req = match body {
            Some(payload) => client.post(&url).json(payload),
            None => client.get(&url),
        };
        let res = req
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
            .map_err(|e| format!("تعذّر الوصول إلى المنصة: {e}"))?;
        let status = res.status();
        let parsed: Value = res
            .json()
            .await
            .map_err(|e| format!("ردٌّ غير مفهوم من المنصة: {e}"))?;
        if status == reqwest::StatusCode::UNAUTHORIZED && attempt == 0 {
            continue;
        }
        if !status.is_success() {
            return Err(platform_error(status, &parsed));
        }
        return Ok((parsed, carried));
    }
    Err("تعذّر تسجيل الدخول إلى المنصة بعد محاولتين.".to_string())
}

// ---------------------------------------------------------------------------
// القراءة والتنفيذ
// ---------------------------------------------------------------------------

/// ما ينتظر منى الآن في المنصة.
pub async fn pending(cfg: &SchoolConfig) -> Result<(Pending, Option<TokenUpdate>), String> {
    let (body, token) = request(cfg, None).await?;
    let parsed: Pending =
        serde_json::from_value(body).map_err(|e| format!("شكلٌ غير متوقَّع من المنصة: {e}"))?;
    Ok((parsed, token))
}

/// تنفيذُ إجراءٍ واحد. **والبصمة تُمرَّر كما وصلت من القائمة** — والمنصة هي
/// التي تُعيد إيجاد الصفّ بها وتتحقّق أن البند لا يزال معلَّقًا، فإن اعتمدته
/// منى بنفسها في الأثناء عاد `already` ولم يُكتب شيء.
pub async fn execute(
    cfg: &SchoolConfig,
    kind: &str,
    match_: &Value,
    row_index_hint: i64,
    note: Option<&str>,
) -> Result<(ExecuteOutcome, Option<TokenUpdate>), String> {
    let mut payload = json!({
        "kind": kind,
        "match": match_,
        "rowIndexHint": row_index_hint,
    });
    if let Some(n) = note.map(str::trim).filter(|n| !n.is_empty()) {
        payload["note"] = json!(n);
    }
    let (body, token) = request(cfg, Some(&payload)).await?;
    let parsed: ExecuteOutcome =
        serde_json::from_value(body).map_err(|e| format!("شكلٌ غير متوقَّع من المنصة: {e}"))?;
    Ok((parsed, token))
}

/// سطرٌ واحد لكل بند، بالترتيب نفسه — يُدسّ في الموجز الصباحي (`brief.rs`)
/// فتقرأ منى **ما ينتظرها في المنصة** مع ما ينتظرها على الماك، في نصٍّ واحد.
pub fn summarize(p: &Pending) -> String {
    if p.items.is_empty() {
        return "المنصة: لا شيء معلَّق عليك الآن.".to_string();
    }
    let mut lines: Vec<String> = Vec::new();
    if p.family_waiting > 0 {
        lines.push(format!(
            "المنصة: {} بندًا تنتظره أسرةٌ أو طفل فعلًا.",
            p.family_waiting
        ));
    } else {
        lines.push("المنصة: لا بند تنتظره أسرة — الباقي مالٌ وورق.".to_string());
    }
    for it in &p.items {
        let doable = it.actions.len();
        let tail = if doable > 0 {
            format!(" — أقدر أنفّذ منها {doable} بكلمتك")
        } else {
            format!(" — من {}", it.to)
        };
        lines.push(format!("• {} ({}){}", it.title, it.count, tail));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../schema.sql")).unwrap();
        conn
    }

    fn sample() -> Pending {
        Pending {
            generated_at: "2026-09-12T20:00:00Z".to_string(),
            family_waiting: 7,
            items: vec![
                PendingItem {
                    id: "plans_pending_approval".to_string(),
                    tier: 0,
                    title: "خطط أسبوعية بانتظار اعتمادك".to_string(),
                    detail: String::new(),
                    count: 6,
                    to: "/admin/approvals".to_string(),
                    names: vec![],
                    actions: vec![PendingAction {
                        kind: "approve_weekly_plan".to_string(),
                        label: "اعتماد خطة".to_string(),
                        row_index_hint: 3,
                        r#match: json!({ "الصف": "KG1-A" }),
                    }],
                },
                PendingItem {
                    id: "staff_files_incomplete".to_string(),
                    tier: 2,
                    title: "ملفات كادر ناقصة".to_string(),
                    detail: String::new(),
                    count: 2,
                    to: "/admin/teachers".to_string(),
                    names: vec![],
                    actions: vec![],
                },
            ],
        }
    }

    #[test]
    fn summary_says_what_can_be_executed_and_where_the_rest_lives() {
        let text = summarize(&sample());
        // العددُ الذي تسمعه أولًا هو **من تنتظره أسرة**، لا عددُ البنود.
        assert!(text.contains("7 بندًا تنتظره أسرةٌ أو طفل"));
        assert!(text.contains("أقدر أنفّذ منها 1"));
        // البند الذي لا إجراء له يُقال **مع شاشته** — وإلا سمعت منى عددًا
        // ولم تعرف من أين تُغلقه.
        assert!(text.contains("/admin/teachers"));
    }

    #[test]
    fn empty_pending_is_said_plainly_not_as_an_empty_list() {
        let p = Pending {
            generated_at: String::new(),
            family_waiting: 0,
            items: vec![],
        };
        assert_eq!(summarize(&p), "المنصة: لا شيء معلَّق عليك الآن.");
    }

    #[test]
    fn base_url_has_no_trailing_slash_so_paths_do_not_double_up() {
        let conn = test_conn();
        set_setting(&conn, SCHOOL_BASE_URL_KEY, "https://example.com/").unwrap();
        assert_eq!(base_url(&conn), "https://example.com");
    }

    #[test]
    fn credentials_are_only_complete_when_both_fields_are_filled() {
        let conn = test_conn();
        assert!(!has_credentials(&conn));
        set_setting(&conn, SCHOOL_EMAIL_KEY, "admin@example.com").unwrap();
        assert!(!has_credentials(&conn));
        set_setting(&conn, SCHOOL_PASSWORD_KEY, "x").unwrap();
        assert!(has_credentials(&conn));
    }

    #[test]
    fn load_config_refuses_before_credentials_are_entered() {
        let conn = test_conn();
        assert!(load_config(&conn).is_err());
        set_setting(&conn, SCHOOL_EMAIL_KEY, "admin@example.com").unwrap();
        set_setting(&conn, SCHOOL_PASSWORD_KEY, "x").unwrap();
        let cfg = load_config(&conn).unwrap();
        assert_eq!(cfg.base_url, DEFAULT_BASE_URL);
        assert!(cfg.token.is_none());
    }

    #[test]
    fn a_token_inside_the_safety_margin_is_not_reused() {
        // رمزٌ يبقى ٣٠ ثانية ليس رمزًا صالحًا: قد ينتهي أثناء الطلب نفسه.
        let conn = test_conn();
        set_setting(&conn, SCHOOL_EMAIL_KEY, "admin@example.com").unwrap();
        set_setting(&conn, SCHOOL_PASSWORD_KEY, "x").unwrap();
        let soon = chrono::Utc::now().timestamp() + 30;
        store_token(
            &conn,
            &TokenUpdate {
                token: "t".to_string(),
                expires_at: soon,
            },
        )
        .unwrap();
        let cfg = load_config(&conn).unwrap();
        assert!(cfg.token_expires_at - chrono::Utc::now().timestamp() <= TOKEN_SAFETY_MARGIN_SECS);
    }

    #[test]
    fn forget_token_keeps_the_credentials_so_she_does_not_retype_them() {
        let conn = test_conn();
        set_setting(&conn, SCHOOL_EMAIL_KEY, "admin@example.com").unwrap();
        set_setting(&conn, SCHOOL_PASSWORD_KEY, "x").unwrap();
        store_token(
            &conn,
            &TokenUpdate {
                token: "t".to_string(),
                expires_at: 99_999_999_999,
            },
        )
        .unwrap();
        forget_token(&conn);
        assert!(has_credentials(&conn));
        assert!(load_config(&conn).unwrap().token.is_none());
    }
}
