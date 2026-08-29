/*
 * اختبار وحدة الوصول المباشر لجوجل شيت. بيتحقق من:
 *  - توقيع JWT بحساب خدمة حقيقي (نولّد زوج مفاتيح RSA فعلي ونتأكد التوقيع
 *    يتحقق بالمفتاح العام) — أهم جزء وأخطره.
 *  - شكل نداءات Sheets API (append/update/delete/read) بـ fetch وهمي.
 *  - بوابة الموافقة: تأكيد على الحسّاس فقط، والحذف دايمًا.
 * مفيش أي اتصال حقيقي بجوجل. يشغّل: node mobile/school-sheets.test.mjs
 */
import { webcrypto } from "node:crypto";
await import("./school-sheets.js");
const S = globalThis.SchoolSheets;

let passed = 0,
  failed = 0;
function ok(name, cond) {
  if (cond) { passed++; console.log("  ✓ " + name); }
  else { failed++; console.error("  ✗ " + name); }
}

// ---- توليد حساب خدمة حقيقي (مفتاح RSA) لاختبار التوقيع فعليًا ----
const pair = await webcrypto.subtle.generateKey(
  { name: "RSASSA-PKCS1-v1_5", modulusLength: 2048, publicExponent: new Uint8Array([1, 0, 1]), hash: "SHA-256" },
  true,
  ["sign", "verify"]
);
const pkcs8 = new Uint8Array(await webcrypto.subtle.exportKey("pkcs8", pair.privateKey));
function toPem(bytes) {
  const b64 = Buffer.from(bytes).toString("base64").replace(/(.{64})/g, "$1\n");
  return "-----BEGIN PRIVATE KEY-----\n" + b64 + "\n-----END PRIVATE KEY-----\n";
}
const SA = { client_email: "amin@durrat.iam.gserviceaccount.com", private_key: toPem(pkcs8) };

function makeFetch(routes) {
  const calls = [];
  const fn = async (url, opts) => {
    calls.push({ url, opts });
    for (const r of routes) {
      if (url.includes(r.match)) {
        return { ok: r.status ? r.status < 400 : true, status: r.status || 200, json: async () => r.body };
      }
    }
    return { ok: false, status: 404, json: async () => ({ error: { message: "no route" } }) };
  };
  return { fn, calls };
}
const TOKEN_OK = { match: "oauth2.googleapis.com/token", body: { access_token: "ACCESS1", expires_in: 3600 } };

async function test_jwt_signing_is_real() {
  console.log("توقيع JWT بحساب خدمة حقيقي يتحقق بالمفتاح العام:");
  S.resetToken();
  let capturedAssertion = "";
  const { fn } = makeFetch([TOKEN_OK]);
  const wrapped = async (url, opts) => {
    if (url.includes("token")) {
      const m = /assertion=([^&]+)/.exec(opts.body);
      capturedAssertion = decodeURIComponent(m[1]);
    }
    return fn(url, opts);
  };
  S.configure({ fetch: wrapped, subtle: webcrypto.subtle, getServiceAccount: () => SA });
  const tok = await S.mintAccessToken();
  ok("رجع access token", tok === "ACCESS1");

  const [h, p, sig] = capturedAssertion.split(".");
  const signingInput = new TextEncoder().encode(h + "." + p);
  const sigBytes = Uint8Array.from(
    Buffer.from(sig.replace(/-/g, "+").replace(/_/g, "/"), "base64")
  );
  const verified = await webcrypto.subtle.verify(
    "RSASSA-PKCS1-v1_5", pair.publicKey, sigBytes, signingInput
  );
  ok("التوقيع صحيح رياضيًا (RS256)", verified === true);
  const claim = JSON.parse(Buffer.from(p.replace(/-/g, "+").replace(/_/g, "/"), "base64").toString());
  ok("المطالبة فيها البريد والنطاق الصحيح",
    claim.iss === SA.client_email && claim.scope.includes("spreadsheets"));
}

async function test_token_cached() {
  console.log("التوكن يُكاش (نداء token واحد لعمليتين):");
  S.resetToken();
  let tokenHits = 0;
  const { fn } = makeFetch([
    TOKEN_OK,
    { match: "/values/", body: { values: [["أ"], ["1"]] } },
  ]);
  const wrapped = async (url, opts) => { if (url.includes("token")) tokenHits++; return fn(url, opts); };
  S.configure({ fetch: wrapped, subtle: webcrypto.subtle, getServiceAccount: () => SA });
  await S.runTool("school_sheet_read", { tab: "الإعلانات" }, () => true);
  await S.runTool("school_sheet_read", { tab: "الإعلانات" }, () => true);
  ok("نداء token مرة واحدة فقط", tokenHits === 1);
}

async function test_read_maps_rows() {
  console.log("القراءة تحوّل الصفوف لكائنات بالرؤوس:");
  S.resetToken();
  const { fn } = makeFetch([
    TOKEN_OK,
    { match: "/values/", body: { values: [["الرقم المدني", "اسم الطالب"], ["123", "سالم"], ["456", "نورة"]] } },
  ]);
  S.configure({ fetch: fn, subtle: webcrypto.subtle, getServiceAccount: () => SA });
  const out = await S.runTool("school_sheet_read", { tab: "الطلاب" }, () => true);
  ok("عدد الصفوف صحيح", out.count === 2);
  ok("التحويل لكائن صحيح", out.rows[1]["اسم الطالب"] === "نورة");
}

async function test_add_maps_to_headers() {
  console.log("الإضافة ترتّب القيم حسب ترتيب الرؤوس:");
  S.resetToken();
  const { fn, calls } = makeFetch([
    TOKEN_OK,
    { match: ":append", body: { updates: { updatedRows: 1 } } },
    { match: "/values/", body: { values: [["الرقم المدني", "اسم الطالب", "الصف"]] } },
  ]);
  S.configure({ fetch: fn, subtle: webcrypto.subtle, getServiceAccount: () => SA });
  // "الطلاب" حسّاس → لازم موافقة
  const out = await S.runTool(
    "school_sheet_add",
    { tab: "الطلاب", row: { "اسم الطالب": "خالد", "الرقم المدني": "999" } },
    () => true
  );
  ok("نجحت الإضافة", out.ok === true);
  const appendCall = calls.find((c) => c.url.includes(":append"));
  const sent = JSON.parse(appendCall.opts.body).values[0];
  ok("القيم مرتبة حسب الرؤوس", sent[0] === "999" && sent[1] === "خالد" && sent[2] === "");
}

async function test_sensitive_add_needs_confirm() {
  console.log("إضافة لتبويب حسّاس تُمنع بدون موافقة:");
  S.resetToken();
  const { fn, calls } = makeFetch([
    TOKEN_OK,
    { match: "/values/", body: { values: [["اسم الطالب"]] } },
    { match: ":append", body: {} },
  ]);
  S.configure({ fetch: fn, subtle: webcrypto.subtle, getServiceAccount: () => SA });
  const out = await S.runTool("school_sheet_add", { tab: "الطلاب", row: { "اسم الطالب": "س" } }, () => false);
  ok("رجعت cancelled", out.cancelled === true);
  ok("لم تُرسل الإضافة", !calls.find((c) => c.url.includes(":append")));
}

async function test_nonsensitive_add_no_confirm() {
  console.log("إضافة لتبويب غير حسّاس تتنفذ بدون موافقة:");
  S.resetToken();
  const { fn, calls } = makeFetch([
    TOKEN_OK,
    { match: "/values/", body: { values: [["التاريخ", "العنوان"]] } },
    { match: ":append", body: { updates: { updatedRows: 1 } } },
  ]);
  S.configure({ fetch: fn, subtle: webcrypto.subtle, getServiceAccount: () => SA });
  let asked = false;
  const out = await S.runTool(
    "school_sheet_add",
    { tab: "الإعلانات", row: { "العنوان": "إجازة" } },
    () => { asked = true; return false; }
  );
  ok("لم تُطلب موافقة (غير حسّاس)", asked === false);
  ok("نُفّذت الإضافة", out.ok === true);
}

async function test_delete_always_confirms() {
  console.log("الحذف يطلب موافقة حتى في تبويب غير حسّاس:");
  ok("needsConfirm للحذف صحيح", S._needsConfirm("school_sheet_delete", { tab: "الإعلانات" }) === true);
  ok("needsConfirm لإضافة غير حسّاس = false", S._needsConfirm("school_sheet_add", { tab: "الإعلانات" }) === false);
  ok("needsConfirm لإضافة حسّاس = true", S._needsConfirm("school_sheet_add", { tab: "الأقساط" }) === true);
}

async function test_delete_bottom_up() {
  console.log("الحذف يمسح من الأسفل للأعلى بأرقام صفوف صحيحة:");
  S.resetToken();
  const { fn, calls } = makeFetch([
    TOKEN_OK,
    { match: "fields=sheets.properties", body: { sheets: [{ properties: { title: "الأقساط", sheetId: 7, gridProperties: { rowCount: 100 } } }] } },
    { match: "/values/", body: { values: [["الحالة"], ["متأخر"], ["مدفوع"], ["متأخر"]] } },
    { match: ":batchUpdate", body: {} },
  ]);
  S.configure({ fetch: fn, subtle: webcrypto.subtle, getServiceAccount: () => SA });
  const out = await S.runTool("school_sheet_delete", { tab: "الأقساط", match: { "الحالة": "متأخر" } }, () => true);
  ok("حذف صفين", out.result.deleted === 2);
  const batch = calls.find((c) => c.url.includes(":batchUpdate"));
  const reqs = JSON.parse(batch.opts.body).requests;
  // الصفوف المطابقة: 2 و 4 (بيانات) → تُحذف 4 ثم 2
  ok("أول حذف للصف الأسفل (index 3)", reqs[0].deleteDimension.range.startIndex === 3);
  ok("ثم الصف الأعلى (index 1)", reqs[1].deleteDimension.range.startIndex === 1);
  ok("على التبويب الصحيح (sheetId)", reqs[0].deleteDimension.range.sheetId === 7);
}

async function test_missing_sa() {
  console.log("بدون مفتاح حساب خدمة: رسالة واضحة:");
  S.resetToken();
  const { fn } = makeFetch([TOKEN_OK]);
  S.configure({ fetch: fn, subtle: webcrypto.subtle, getServiceAccount: () => null });
  const out = await S.runTool("school_sheet_tabs", {}, () => true);
  ok("رسالة تطلب مفتاح حساب الخدمة", typeof out.error === "string" && out.error.includes("حساب خدمة"));
}

function test_defs_parity() {
  console.log("تعريفات الأدوات متسقة مع المنفّذات:");
  const defs = S.TOOL_DEFS.map((d) => d.name).sort();
  const impl = Object.keys(S._TOOLS).sort();
  ok("تطابق التعريف والمنفّذ", JSON.stringify(defs) === JSON.stringify(impl));
}

const tests = [
  test_jwt_signing_is_real,
  test_token_cached,
  test_read_maps_rows,
  test_add_maps_to_headers,
  test_sensitive_add_needs_confirm,
  test_nonsensitive_add_no_confirm,
  test_delete_always_confirms,
  test_delete_bottom_up,
  test_missing_sa,
  test_defs_parity,
];
for (const t of tests) await t();
console.log("\nالنتيجة: " + passed + " ناجح، " + failed + " فاشل");
process.exit(failed ? 1 : 0);
