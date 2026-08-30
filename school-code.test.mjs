/*
 * اختبار وحدة كود المنصة — بـ fetch وهمي (مفيش اتصال حقيقي بـGitHub):
 *  - القراءة تفكّ base64 بعربي صحيح.
 *  - proposeFix بيعمل السلسلة الصح: فرع افتراضي ← ref ← PUT ← PR، ويرجّع
 *    رابط الـPR، ولا يلمس الفرع الحيّ.
 *  - بوابة الموافقة: الاقتراح (كتابة) يُمنع بدون موافقة.
 *  - base64 عربي ذهاب وعودة.
 * يشغّل: node mobile/school-code.test.mjs
 */
await import("./school-code.js");
const S = globalThis.SchoolCode;

let passed = 0, failed = 0;
function ok(name, cond) {
  if (cond) { passed++; console.log("  ✓ " + name); }
  else { failed++; console.error("  ✗ " + name); }
}

function makeFetch(routes) {
  const calls = [];
  const fn = async (url, opts) => {
    calls.push({ url, opts, method: (opts && opts.method) || "GET" });
    for (const r of routes) {
      if ((!r.method || r.method === ((opts && opts.method) || "GET")) && url.includes(r.match)) {
        return { ok: r.status ? r.status < 400 : true, status: r.status || 200, json: async () => r.body };
      }
    }
    return { ok: false, status: 404, json: async () => ({ message: "no route: " + url }) };
  };
  return { fn, calls };
}

function b64(str) { return S._b64.enc(str); }

async function test_base64_roundtrip() {
  console.log("base64 عربي ذهاب وعودة:");
  const s = "دالة إصلاح — رقم مدني ١٢٣";
  ok("النص يرجع كما هو", S._b64.dec(S._b64.enc(s)) === s);
}

async function test_read_file_decodes() {
  console.log("قراءة ملف تفكّ المحتوى:");
  const { fn } = makeFetch([
    { match: "/contents/src/app.ts", body: { type: "file", path: "src/app.ts", sha: "abc", content: b64("export const x = 1; // مرحبا") } },
  ]);
  S.configure({ fetch: fn, getToken: () => "ghp_test" });
  const out = await S.runTool("code_read_file", { path: "src/app.ts" }, () => true);
  ok("رجع المحتوى مفكوكًا", out.ok && out.content.includes("مرحبا") && out.content.includes("export const x"));
}

async function test_list_files() {
  console.log("سرد الملفات:");
  const { fn } = makeFetch([
    { match: "/contents/src", body: [{ name: "app.ts", path: "src/app.ts", type: "file" }, { name: "lib", path: "src/lib", type: "dir" }] },
  ]);
  S.configure({ fetch: fn, getToken: () => "ghp_test" });
  const out = await S.runTool("code_list_files", { path: "src" }, () => true);
  ok("رجعت المدخلات", out.ok && out.entries.length === 2 && out.entries[1].type === "dir");
}

async function test_propose_fix_flow() {
  console.log("اقتراح إصلاح: السلسلة الصحيحة ورابط PR:");
  // المسارات المحددة أولًا؛ المسار العام للمستودع آخرًا (مطابقة أول تطابق).
  const { fn, calls } = makeFetch([
    { match: "/git/ref/heads/main", method: "GET", body: { object: { sha: "BASESHA" } } },
    { match: "/git/refs", method: "POST", body: { ref: "refs/heads/amin-fix" } },
    { match: "/contents/src/bug.ts", method: "GET", body: { type: "file", path: "src/bug.ts", sha: "OLDSHA", content: b64("old") } },
    { match: "/contents/src/bug.ts", method: "PUT", body: { commit: { sha: "NEW" } } },
    { match: "/pulls", method: "POST", body: { number: 42, html_url: "https://github.com/x/y/pull/42" } },
    { match: "/repos/missmonabakry2580-sudo/durrat-bayaan-connect", method: "GET", body: { default_branch: "main" } },
  ]);
  S.configure({ fetch: fn, getToken: () => "ghp_test" });
  const out = await S.runTool(
    "code_propose_fix",
    { path: "src/bug.ts", content: "export const fixed = true;", pr_title: "إصلاح", pr_body: "شرح" },
    () => true
  );
  ok("رجع رقم ورابط الـPR", out.ok && out.result.pr_number === 42 && /pull\/42/.test(out.result.pr_url));

  const refCall = calls.find((c) => c.url.includes("/git/refs") && c.method === "POST");
  ok("فرّع من BASESHA", JSON.parse(refCall.opts.body).sha === "BASESHA");
  const putCall = calls.find((c) => c.url.includes("/contents/src/bug.ts") && c.method === "PUT");
  const putBody = JSON.parse(putCall.opts.body);
  ok("PUT على الفرع الجديد مش الحيّ", /^amin-fix-/.test(putBody.branch));
  ok("PUT حمل sha للملف القديم", putBody.sha === "OLDSHA");
  ok("المحتوى الجديد مُرسل base64", S._b64.dec(putBody.content) === "export const fixed = true;");
  const prCall = calls.find((c) => c.url.includes("/pulls") && c.method === "POST");
  ok("PR من الفرع الجديد إلى main", JSON.parse(prCall.opts.body).base === "main");
}

async function test_write_needs_confirm() {
  console.log("الاقتراح يُمنع بدون موافقة ولا يلمس GitHub:");
  const { fn, calls } = makeFetch([
    { match: "/repos/", body: { default_branch: "main" } },
  ]);
  S.configure({ fetch: fn, getToken: () => "ghp_test" });
  const out = await S.runTool("code_propose_fix", { path: "src/x.ts", content: "y" }, () => false);
  ok("رجعت cancelled", out.cancelled === true);
  ok("لم يُنادَ GitHub إطلاقًا", calls.length === 0);
}

async function test_missing_token() {
  console.log("بدون مفتاح: رسالة واضحة:");
  const { fn } = makeFetch([]);
  S.configure({ fetch: fn, getToken: () => "" });
  const out = await S.runTool("code_read_file", { path: "a" }, () => true);
  ok("رسالة تطلب مفتاح GitHub", typeof out.error === "string" && out.error.includes("مفتاح GitHub"));
}

function test_defs_parity() {
  console.log("تعريفات الأدوات متسقة:");
  const defs = S.TOOL_DEFS.map((d) => d.name).sort();
  const impl = Object.keys(S._TOOLS).sort();
  ok("تطابق التعريف والمنفّذ", JSON.stringify(defs) === JSON.stringify(impl));
}

const tests = [
  test_base64_roundtrip,
  test_read_file_decodes,
  test_list_files,
  test_propose_fix_flow,
  test_write_needs_confirm,
  test_missing_token,
  test_defs_parity,
];
for (const t of tests) await t();
console.log("\nالنتيجة: " + passed + " ناجح، " + failed + " فاشل");
process.exit(failed ? 1 : 0);
