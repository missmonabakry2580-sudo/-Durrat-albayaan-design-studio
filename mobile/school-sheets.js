/*
 * وصول أمين المباشر لقاعدة بيانات درة البيان (Google Sheet) — القراءة
 * والكتابة الكاملة لكل التبويبات، مستقل تمامًا عن منصة المدرسة (مش بيعتمد
 * على بوابة Lovable الداخلية الهشّة). أمين بيتكلم مع Google Sheets API
 * مباشرة بحساب خدمة (service account) منى بتشاركه الشيت.
 *
 * قاعدة الأمان (اختيار منى: تأكيد على الحسّاس فقط): القراءة فورية؛ الإضافة
 * والتعديل على تبويب حسّاس (طلاب/مالية/بنكي/صحة) لازم موافقة؛ والحذف دايمًا
 * لازم موافقة مهما كان التبويب (الحذف لا رجعة فيه). التبويبات غير الحسّاسة
 * (إعلانات، خطط دروس…) الكتابة فيها فورية.
 *
 * الوحدة قابلة للحقن (configure) عشان تتجرب في Node وتشتغل في المتصفح بنفس
 * الكود. مفتاح حساب الخدمة سرّ حقيقي — بيتخزّن على الجهاز فقط، وعمره ما
 * بيتكتب في الكود أو الريبو.
 */
(function () {
  "use strict";

  var SHEET_ID = "1-4aPMj2aLDAhm2wggCMN98fknFriejjWN7YC8QJdYXc";
  var SHEETS_API = "https://sheets.googleapis.com/v4/spreadsheets/" + SHEET_ID;
  var TOKEN_URL = "https://oauth2.googleapis.com/token";
  var SCOPE = "https://www.googleapis.com/auth/spreadsheets";
  var READ_RANGE = "A1:BZ";

  // التبويبات الحسّاسة — الكتابة فيها تعدي على تأكيد. (عناوين حقيقية من
  // مخطط المنصة config.ts.)
  var SENSITIVE_TABS = [
    "الطلاب",
    "الأقساط",
    "الفواتير",
    "الرسوم المستحقة",
    "خطط الرسوم",
    "إثباتات الدفع",
    "الخصومات والمنح",
    "الزي والرحلات",
    "الحسابات البنكية",
    "المتابعة اليومية للطفل",
    "التقارير التطويرية",
    "الدرجات",
  ];

  var deps = {
    fetch: typeof fetch !== "undefined" ? fetch.bind(globalThis) : null,
    subtle: typeof crypto !== "undefined" && crypto.subtle ? crypto.subtle : null,
    // بيرجّع مفتاح حساب الخدمة {client_email, private_key} أو null.
    getServiceAccount: function () {
      return null;
    },
  };
  function configure(opts) {
    if (opts && opts.fetch) deps.fetch = opts.fetch;
    if (opts && opts.subtle) deps.subtle = opts.subtle;
    if (opts && opts.getServiceAccount) deps.getServiceAccount = opts.getServiceAccount;
  }

  // fetch بمهلة زمنية — أي نداء يتعلّق يفشل بسرعة بدل ما يجمّد أمين لدقايق
  // (ده كان سبب "بيتأخر كتير ومش بيرد المرة التانية"). 25 ثانية سقف واقعي.
  function tfetch(url, opts) {
    var ctrl = typeof AbortController !== "undefined" ? new AbortController() : null;
    var timer = ctrl ? setTimeout(function () { ctrl.abort(); }, 25000) : null;
    var o = Object.assign({}, opts || {}, ctrl ? { signal: ctrl.signal } : {});
    return Promise.resolve(deps.fetch(url, o)).then(
      function (r) { if (timer) clearTimeout(timer); return r; },
      function (e) {
        if (timer) clearTimeout(timer);
        if (e && e.name === "AbortError") throw new Error("انتهت مهلة الاتصال بجوجل — جرّبي تاني.");
        throw e;
      }
    );
  }

  /* ---------------- مصادقة حساب الخدمة (JWT RS256 → access token) ------- */
  var token = { value: "", expiresAt: 0 };
  function resetToken() {
    token = { value: "", expiresAt: 0 };
  }

  function b64url(bytes) {
    var bin = "";
    var arr = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
    for (var i = 0; i < arr.length; i++) bin += String.fromCharCode(arr[i]);
    var b64 = typeof btoa !== "undefined" ? btoa(bin) : Buffer.from(bin, "binary").toString("base64");
    return b64.replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  }
  function b64urlStr(str) {
    return b64url(new TextEncoder().encode(str));
  }

  // PEM (PKCS8) → مفتاح توقيع RS256.
  function pemToPkcs8(pem) {
    var body = pem
      .replace(/-----BEGIN [^-]+-----/g, "")
      .replace(/-----END [^-]+-----/g, "")
      .replace(/\s+/g, "");
    var bin = typeof atob !== "undefined" ? atob(body) : Buffer.from(body, "base64").toString("binary");
    var buf = new Uint8Array(bin.length);
    for (var i = 0; i < bin.length; i++) buf[i] = bin.charCodeAt(i);
    return buf.buffer;
  }

  async function mintAccessToken() {
    var now = Math.floor(Date.now() / 1000);
    if (token.value && now < token.expiresAt - 60) return token.value;

    var sa = deps.getServiceAccount();
    if (!sa || !sa.client_email || !sa.private_key) {
      throw new Error(
        "محتاجة تحطي مفتاح حساب خدمة جوجل (Service Account) في إعدادات أمين الأول."
      );
    }
    var header = { alg: "RS256", typ: "JWT" };
    var claim = {
      iss: sa.client_email,
      scope: SCOPE,
      aud: TOKEN_URL,
      iat: now,
      exp: now + 3600,
    };
    var signingInput = b64urlStr(JSON.stringify(header)) + "." + b64urlStr(JSON.stringify(claim));

    if (!deps.subtle) throw new Error("التشفير غير متاح في هذه البيئة.");
    var key = await deps.subtle.importKey(
      "pkcs8",
      pemToPkcs8(sa.private_key),
      { name: "RSASSA-PKCS1-v1_5", hash: "SHA-256" },
      false,
      ["sign"]
    );
    var sig = await deps.subtle.sign(
      "RSASSA-PKCS1-v1_5",
      key,
      new TextEncoder().encode(signingInput)
    );
    var jwt = signingInput + "." + b64url(sig);

    var res = await tfetch(TOKEN_URL, {
      method: "POST",
      headers: { "Content-Type": "application/x-www-form-urlencoded" },
      body:
        "grant_type=urn:ietf:params:oauth:grant-type:jwt-bearer&assertion=" +
        encodeURIComponent(jwt),
    });
    var body = await res.json().catch(function () {
      return {};
    });
    if (!res.ok || !body.access_token) {
      throw new Error(
        "فشل مصادقة حساب الخدمة: " + (body.error_description || body.error || res.status)
      );
    }
    token.value = body.access_token;
    token.expiresAt = now + Number(body.expires_in || 3600);
    return token.value;
  }

  async function api(path, opts) {
    var t = await mintAccessToken();
    opts = opts || {};
    var res = await tfetch(SHEETS_API + path, {
      method: opts.method || "GET",
      headers: Object.assign(
        { Authorization: "Bearer " + t },
        opts.body ? { "Content-Type": "application/json" } : {}
      ),
      body: opts.body ? JSON.stringify(opts.body) : undefined,
    });
    var body = await res.json().catch(function () {
      return {};
    });
    if (!res.ok) {
      if (res.status === 401) resetToken();
      var msg = (body.error && body.error.message) || res.status;
      throw new Error("Google Sheets: " + msg);
    }
    return body;
  }

  /* -------------------------- عمليات الشيت ----------------------------- */
  function q(tab) {
    return encodeURIComponent("'" + tab.replace(/'/g, "''") + "'!" + READ_RANGE);
  }

  async function listTabs() {
    var body = await api("?fields=sheets.properties(title,sheetId,gridProperties(rowCount))");
    return (body.sheets || []).map(function (s) {
      return {
        title: s.properties.title,
        sheetId: s.properties.sheetId,
        rows: (s.properties.gridProperties && s.properties.gridProperties.rowCount) || 0,
      };
    });
  }

  // بيرجّع {headers, rows} حيث rows مصفوفة كائنات + رقم الصف الفعلي بالشيت.
  async function readTab(tab) {
    var body = await api("/values/" + q(tab) + "?majorDimension=ROWS");
    var values = body.values || [];
    var headers = values[0] || [];
    var rows = values.slice(1).map(function (r, idx) {
      var obj = {};
      for (var c = 0; c < headers.length; c++) obj[headers[c]] = r[c] != null ? r[c] : "";
      return { _row: idx + 2, data: obj };
    });
    return { headers: headers, rows: rows };
  }

  function matchRow(rowData, match) {
    if (!match) return true;
    return Object.keys(match).every(function (col) {
      var v = String(rowData[col] != null ? rowData[col] : "").trim();
      var want = String(match[col]).trim();
      return v === want;
    });
  }

  async function findRows(tab, match) {
    var t = await readTab(tab);
    return t.rows.filter(function (r) {
      return matchRow(r.data, match);
    });
  }

  async function appendRow(tab, rowObj) {
    var t = await readTab(tab);
    var line = t.headers.map(function (h) {
      return rowObj[h] != null ? String(rowObj[h]) : "";
    });
    return api("/values/" + q(tab) + ":append?valueInputOption=USER_ENTERED", {
      method: "POST",
      body: { values: [line] },
    });
  }

  // تعديل الصفوف المطابقة: بيدمج changes فوق كل صف مطابق ويكتبه كامل.
  async function updateRows(tab, match, changes) {
    var t = await readTab(tab);
    var targets = t.rows.filter(function (r) {
      return matchRow(r.data, match);
    });
    if (targets.length === 0) return { updated: 0, note: "لا صفوف مطابقة" };
    var dataUpdates = targets.map(function (r) {
      var merged = Object.assign({}, r.data, changes);
      var line = t.headers.map(function (h) {
        return merged[h] != null ? String(merged[h]) : "";
      });
      return {
        range: "'" + tab.replace(/'/g, "''") + "'!A" + r._row,
        majorDimension: "ROWS",
        values: [line],
      };
    });
    await api("/values:batchUpdate", {
      method: "POST",
      body: { valueInputOption: "USER_ENTERED", data: dataUpdates },
    });
    return { updated: targets.length, rows: targets.map(function (r) { return r._row; }) };
  }

  async function deleteRows(tab, match) {
    var tabs = await listTabs();
    var meta = tabs.find(function (s) {
      return s.title === tab;
    });
    if (!meta) throw new Error("تبويب غير موجود: " + tab);
    var targets = await findRows(tab, match);
    if (targets.length === 0) return { deleted: 0, note: "لا صفوف مطابقة" };
    // نحذف من الأسفل للأعلى عشان أرقام الصفوف ما تزحفش.
    var rowsDesc = targets
      .map(function (r) { return r._row; })
      .sort(function (a, b) { return b - a; });
    var requests = rowsDesc.map(function (rowNum) {
      return {
        deleteDimension: {
          range: {
            sheetId: meta.sheetId,
            dimension: "ROWS",
            startIndex: rowNum - 1,
            endIndex: rowNum,
          },
        },
      };
    });
    await api(":batchUpdate", { method: "POST", body: { requests: requests } });
    return { deleted: targets.length, rows: rowsDesc };
  }

  /* ------------------------- أدوات أمين -------------------------------- */
  function isSensitiveTab(tab) {
    return SENSITIVE_TABS.indexOf(tab) !== -1;
  }

  var TOOLS = {
    school_sheet_tabs: { kind: "read" },
    school_sheet_read: { kind: "read" },
    school_sheet_find: { kind: "read" },
    school_sheet_add: { kind: "write" },
    school_sheet_update: { kind: "write" },
    school_sheet_delete: { kind: "write" },
  };

  var TOOL_DEFS = [
    {
      name: "school_sheet_tabs",
      description:
        "اسرد كل تبويبات قاعدة بيانات المدرسة (الجوجل شيت) وعدد صفوف كل واحد. قراءة.",
      input_schema: { type: "object", properties: {} },
    },
    {
      name: "school_sheet_read",
      description:
        "اقرأ صفوف تبويب كامل من قاعدة البيانات (مثلًا «الطلاب» أو «الأقساط»). قراءة.",
      input_schema: {
        type: "object",
        properties: { tab: { type: "string", description: "اسم التبويب بالعربي" } },
        required: ["tab"],
      },
    },
    {
      name: "school_sheet_find",
      description:
        "ابحث عن صفوف في تبويب تطابق قيم أعمدة معيّنة (مثلًا الرقم المدني=xxxx). قراءة.",
      input_schema: {
        type: "object",
        properties: {
          tab: { type: "string" },
          match: { type: "object", description: "خريطة اسم عمود ← قيمة مطلوبة" },
        },
        required: ["tab", "match"],
      },
    },
    {
      name: "school_sheet_add",
      description:
        "أضف صفًا جديدًا لتبويب. القيم خريطة اسم عمود ← قيمة. يتطلب موافقة منى لو التبويب حسّاس.",
      input_schema: {
        type: "object",
        properties: {
          tab: { type: "string" },
          row: { type: "object", description: "خريطة اسم عمود ← قيمة" },
        },
        required: ["tab", "row"],
      },
    },
    {
      name: "school_sheet_update",
      description:
        "عدّل الصفوف المطابقة في تبويب. match يحدد الصفوف، changes القيم الجديدة. يتطلب موافقة لو التبويب حسّاس.",
      input_schema: {
        type: "object",
        properties: {
          tab: { type: "string" },
          match: { type: "object" },
          changes: { type: "object" },
        },
        required: ["tab", "match", "changes"],
      },
    },
    {
      name: "school_sheet_delete",
      description:
        "احذف الصفوف المطابقة في تبويب نهائيًا. يتطلب موافقة منى دائمًا (لا رجعة).",
      input_schema: {
        type: "object",
        properties: { tab: { type: "string" }, match: { type: "object" } },
        required: ["tab", "match"],
      },
    },
  ];

  function isSchoolSheetTool(name) {
    return Object.prototype.hasOwnProperty.call(TOOLS, name);
  }

  function needsConfirm(name, input) {
    if (TOOLS[name] && TOOLS[name].kind !== "write") return false;
    if (name === "school_sheet_delete") return true; // الحذف دايمًا
    return isSensitiveTab(input.tab); // إضافة/تعديل: الحسّاس فقط
  }

  function summarize(name, input) {
    if (name === "school_sheet_delete")
      return "حذف صفوف مطابقة (" + JSON.stringify(input.match) + ") من «" + input.tab + "» نهائيًا؟";
    if (name === "school_sheet_add")
      return "إضافة صف جديد لـ«" + input.tab + "»؟";
    if (name === "school_sheet_update")
      return "تعديل صفوف مطابقة في «" + input.tab + "» بالقيم " + JSON.stringify(input.changes) + "؟";
    return "تنفيذ " + name + " على «" + input.tab + "»؟";
  }

  async function runTool(name, input, confirmFn) {
    if (!isSchoolSheetTool(name)) return { error: "أداة غير معروفة: " + name };
    input = input || {};

    if (needsConfirm(name, input)) {
      var ok = false;
      try {
        ok = confirmFn ? await confirmFn(summarize(name, input), name, input) : false;
      } catch (_e) {
        ok = false;
      }
      if (!ok) return { cancelled: true, note: "منى لم توافق على: " + summarize(name, input) };
    }

    try {
      switch (name) {
        case "school_sheet_tabs":
          return { ok: true, tabs: await listTabs() };
        case "school_sheet_read": {
          var t = await readTab(input.tab);
          return { ok: true, headers: t.headers, rows: t.rows.map(function (r) { return r.data; }), count: t.rows.length };
        }
        case "school_sheet_find": {
          var found = await findRows(input.tab, input.match);
          return { ok: true, rows: found.map(function (r) { return r.data; }), count: found.length };
        }
        case "school_sheet_add":
          return { ok: true, result: await appendRow(input.tab, input.row || {}) };
        case "school_sheet_update":
          return { ok: true, result: await updateRows(input.tab, input.match, input.changes || {}) };
        case "school_sheet_delete":
          return { ok: true, result: await deleteRows(input.tab, input.match) };
        default:
          return { error: "أداة غير مدعومة: " + name };
      }
    } catch (e) {
      return { error: String((e && e.message) || e) };
    }
  }

  var SchoolSheets = {
    configure: configure,
    isSchoolSheetTool: isSchoolSheetTool,
    runTool: runTool,
    resetToken: resetToken,
    mintAccessToken: mintAccessToken,
    TOOL_DEFS: TOOL_DEFS,
    SENSITIVE_TABS: SENSITIVE_TABS,
    _needsConfirm: needsConfirm,
    _TOOLS: TOOLS,
    _SHEET_ID: SHEET_ID,
  };

  if (typeof module !== "undefined" && module.exports) module.exports = SchoolSheets;
  if (typeof globalThis !== "undefined") globalThis.SchoolSheets = SchoolSheets;
})();
