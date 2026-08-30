/*
 * اختبار وحدة أدوات بوابة الإدارة — بيتحقق من المنطق الحساس (تسجيل الدخول،
 * كاش التوكن، شكل نداء الدالة، وبوابة الموافقة) بـ fetch وهمي، من غير ما
 * يلمس المنصة الحقيقية. يشغّل: node mobile/school-admin.test.mjs
 */
// الملف ESM-side-effect: بيسجّل نفسه على globalThis (زي المتصفح على window).
await import("./school-admin.js");
const SchoolAdmin = globalThis.SchoolAdmin;

let passed = 0,
  failed = 0;
function ok(name, cond) {
  if (cond) {
    passed++;
    console.log("  ✓ " + name);
  } else {
    failed++;
    console.error("  ✗ " + name);
  }
}

/** fetch وهمي: بيسجّل كل النداءات وبيرد ردودًا مبرمجة حسب الرابط. */
function makeFetch(routes) {
  const calls = [];
  const fn = async (url, opts) => {
    calls.push({ url, opts });
    for (const r of routes) {
      if (url.includes(r.match)) {
        return {
          ok: r.status ? r.status < 400 : true,
          status: r.status || 200,
          json: async () => r.body,
        };
      }
    }
    return { ok: false, status: 404, json: async () => ({ error: { message: "no route" } }) };
  };
  return { fn, calls };
}

const GOOD_SIGNIN = {
  match: "signInWithPassword",
  body: { idToken: "TOK1", refreshToken: "REF1", expiresIn: "3600" },
};

async function test_signin_caches_token() {
  console.log("تسجيل الدخول يكاش التوكن ولا يعيد الدخول:");
  SchoolAdmin.resetSession();
  const signinHits = [];
  const { fn } = makeFetch([
    { match: "signInWithPassword", body: { idToken: "TOK1", refreshToken: "REF1", expiresIn: "3600" } },
    { match: "adminListPortalAccounts", body: { result: { accounts: [] } } },
  ]);
  const wrapped = async (url, opts) => {
    if (url.includes("signInWithPassword")) signinHits.push(1);
    return fn(url, opts);
  };
  SchoolAdmin.configure({
    fetch: wrapped,
    getCred: () => ({ email: "a@b.com", password: "pw123456" }),
  });
  await SchoolAdmin.callFunction("adminListPortalAccounts", { role: "parent" });
  await SchoolAdmin.callFunction("adminListPortalAccounts", { role: "student" });
  ok("سجّل الدخول مرة واحدة فقط للنداءين", signinHits.length === 1);
}

async function test_callfunction_shape() {
  console.log("نداء الدالة بالشكل الصحيح (URL + Bearer + {data}):");
  SchoolAdmin.resetSession();
  const { fn, calls } = makeFetch([
    GOOD_SIGNIN,
    { match: "adminSetPortalStatus", body: { result: { updated: true } } },
  ]);
  SchoolAdmin.configure({ fetch: fn, getCred: () => ({ email: "a@b.com", password: "pw123456" }) });
  const res = await SchoolAdmin.callFunction("adminSetPortalStatus", { civilId: "123", status: "disabled" });
  const call = calls.find((c) => c.url.includes("adminSetPortalStatus"));
  ok("الرابط لمنطقة europe-west1 والمشروع الصحيح",
    call.url === "https://europe-west1-durrat-al-bayaan-portal.cloudfunctions.net/adminSetPortalStatus");
  ok("ترويسة Authorization فيها التوكن", call.opts.headers.Authorization === "Bearer TOK1");
  ok("الجسم ملفوف في {data}", JSON.parse(call.opts.body).data.status === "disabled");
  ok("الرد مفكوك من {result}", res.updated === true);
}

async function test_read_no_confirm() {
  console.log("أداة القراءة تتنفذ بدون موافقة:");
  SchoolAdmin.resetSession();
  const { fn, calls } = makeFetch([
    GOOD_SIGNIN,
    { match: "searchPlatformUsers", body: { result: { items: [{ name: "x" }] } } },
  ]);
  SchoolAdmin.configure({ fetch: fn, getCred: () => ({ email: "a@b.com", password: "pw123456" }) });
  let confirmCalled = false;
  const out = await SchoolAdmin.runTool("school_search_users", { query: "منى" }, () => {
    confirmCalled = true;
    return false;
  });
  ok("لم تُطلب الموافقة للقراءة", confirmCalled === false);
  ok("رجعت نتيجة البحث", out.ok && out.result.items.length === 1);
  ok("نادت الدالة فعلًا", !!calls.find((c) => c.url.includes("searchPlatformUsers")));
}

async function test_write_blocked_without_confirm() {
  console.log("أداة الكتابة تُمنع بدون موافقة — ولا تلمس المنصة:");
  SchoolAdmin.resetSession();
  const { fn, calls } = makeFetch([
    GOOD_SIGNIN,
    { match: "adminDeletePortalAccount", body: { result: { deleted: true } } },
  ]);
  SchoolAdmin.configure({ fetch: fn, getCred: () => ({ email: "a@b.com", password: "pw123456" }) });
  const out = await SchoolAdmin.runTool("school_delete_portal_account", { civilId: "999" }, () => false);
  ok("رجعت cancelled", out.cancelled === true);
  ok("لم تنادِ دالة الحذف إطلاقًا", !calls.find((c) => c.url.includes("adminDeletePortalAccount")));
}

async function test_write_runs_with_confirm() {
  console.log("أداة الكتابة تُنفَّذ بعد الموافقة — والملخص صحيح:");
  SchoolAdmin.resetSession();
  const { fn, calls } = makeFetch([
    GOOD_SIGNIN,
    { match: "adminCreatePortalAccount", body: { result: { civilId: "555" } } },
  ]);
  SchoolAdmin.configure({ fetch: fn, getCred: () => ({ email: "a@b.com", password: "pw123456" }) });
  let seenSummary = "";
  const out = await SchoolAdmin.runTool(
    "school_create_portal_account",
    { civilId: "555", role: "parent", displayName: "أم أحمد" },
    (summary) => {
      seenSummary = summary;
      return true;
    }
  );
  ok("الملخص يذكر الاسم والرقم", seenSummary.includes("أم أحمد") && seenSummary.includes("555"));
  ok("نُفّذت وأرجعت النتيجة", out.ok && out.result.civilId === "555");
  const call = calls.find((c) => c.url.includes("adminCreatePortalAccount"));
  ok("أُرسل الدور الصحيح", JSON.parse(call.opts.body).data.role === "parent");
}

async function test_function_error_surfaced() {
  console.log("خطأ الدالة (مثلًا صلاحية) يظهر بوضوح:");
  SchoolAdmin.resetSession();
  const { fn } = makeFetch([
    GOOD_SIGNIN,
    {
      match: "adminSetPortalStatus",
      status: 403,
      body: { error: { message: "هذه العملية متاحة للإدارة فقط.", status: "PERMISSION_DENIED" } },
    },
  ]);
  SchoolAdmin.configure({ fetch: fn, getCred: () => ({ email: "a@b.com", password: "pw123456" }) });
  const out = await SchoolAdmin.runTool(
    "school_set_portal_status",
    { civilId: "1", status: "active" },
    () => true
  );
  ok("رجع خطأ مقروء", typeof out.error === "string" && out.error.includes("للإدارة فقط"));
}

async function test_missing_creds() {
  console.log("بدون بيانات دخول: رسالة واضحة لا انهيار:");
  SchoolAdmin.resetSession();
  const { fn } = makeFetch([GOOD_SIGNIN]);
  SchoolAdmin.configure({ fetch: fn, getCred: () => ({ email: "", password: "" }) });
  const out = await SchoolAdmin.runTool("school_search_users", { query: "x" }, () => true);
  ok("رسالة تطلب بيانات الإدارة", typeof out.error === "string" && out.error.includes("حساب الإدارة"));
}

function test_tool_defs_consistent() {
  console.log("تعريفات الأدوات متسقة مع منفّذاتها:");
  const defNames = SchoolAdmin.TOOL_DEFS.map((d) => d.name).sort();
  const implNames = Object.keys(SchoolAdmin._TOOLS).sort();
  ok("كل أداة معرّفة لها منفّذ والعكس", JSON.stringify(defNames) === JSON.stringify(implNames));
}

const tests = [
  test_signin_caches_token,
  test_callfunction_shape,
  test_read_no_confirm,
  test_write_blocked_without_confirm,
  test_write_runs_with_confirm,
  test_function_error_surfaced,
  test_missing_creds,
  test_tool_defs_consistent,
];

for (const t of tests) await t();
console.log("\nالنتيجة: " + passed + " ناجح، " + failed + " فاشل");
process.exit(failed ? 1 : 0);
