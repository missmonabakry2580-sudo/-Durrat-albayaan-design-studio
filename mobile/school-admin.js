/*
 * أدوات بوابة إدارة درة البيان لأمين — النموذج الأول.
 *
 * أمين بيتكلم مع منصة المدرسة (durrat-al-bayaan-portal على Firebase) عن
 * طريق دوالها الخلفية الجاهزة (Cloud Functions) بصلاحية حساب منى نفسها
 * كأدمن — نفس المنطق المؤمّن اللي المنصة بتستخدمه، مفيش مفتاح خارق بيتخطى
 * القواعد. كل إجراء بيغيّر بيانات لازم يعدي على موافقة صريحة (confirm)
 * قبل ما يتنفذ؛ القراءة فورية. ده تطبيق حرفي لشرط منى: "بينفذ كل شي بموافقة
 * مني".
 *
 * الوحدة مكتوبة عشان تتجرب في Node (بحقن fetch وهمي) وتشتغل في المتصفح
 * (window.SchoolAdmin) بنفس الكود — عشان الصح يتأكد قبل ما يلمس بيانات حقيقية.
 */
(function () {
  "use strict";

  // ثوابت المشروع — مفتاح الويب "منشور" وآمن في المتصفح (تعليق فريق المنصة
  // نفسه في src/lib/firebase/client.ts: التحكم الحقيقي في قواعد Firestore
  // ودوال requireAdmin، مش في سرية المفتاح).
  var PROJECT_ID = "durrat-al-bayaan-portal";
  var WEB_API_KEY = "AIzaSyDxRZhnNber1MMqCe_Y-nEbr4GI0lzTE6g";
  var FUNCTIONS_REGION = "europe-west1";
  var FUNCTIONS_BASE =
    "https://" + FUNCTIONS_REGION + "-" + PROJECT_ID + ".cloudfunctions.net/";
  var SIGNIN_URL =
    "https://identitytoolkit.googleapis.com/v1/accounts:signInWithPassword?key=" +
    WEB_API_KEY;
  var REFRESH_URL = "https://securetoken.googleapis.com/v1/token?key=" + WEB_API_KEY;

  // حُقنة الاعتماديات: fetch وقارئ بيانات الدخول (بريد/كلمة سر الأدمن).
  // في المتصفح بتيجي من localStorage؛ في الاختبار بتتحقن وهمية.
  var deps = {
    fetch: typeof fetch !== "undefined" ? fetch.bind(globalThis) : null,
    getCred: function () {
      return { email: "", password: "" };
    },
  };
  function configure(opts) {
    if (opts && opts.fetch) deps.fetch = opts.fetch;
    if (opts && opts.getCred) deps.getCred = opts.getCred;
  }

  // fetch بمهلة زمنية — أي نداء يتعلّق يفشل بسرعة بدل ما يجمّد أمين.
  function tfetch(url, opts) {
    var ctrl = typeof AbortController !== "undefined" ? new AbortController() : null;
    var timer = ctrl ? setTimeout(function () { ctrl.abort(); }, 25000) : null;
    var o = Object.assign({}, opts || {}, ctrl ? { signal: ctrl.signal } : {});
    return Promise.resolve(deps.fetch(url, o)).then(
      function (r) { if (timer) clearTimeout(timer); return r; },
      function (e) {
        if (timer) clearTimeout(timer);
        if (e && e.name === "AbortError") throw new Error("انتهت مهلة الاتصال — جرّبي تاني.");
        throw e;
      }
    );
  }

  // كاش التوكن في الذاكرة — عمر التوكن ساعة؛ بنجدده بالـ refresh token قبل
  // ما يخلص بدقيقة، وبنعمل دخول جديد لو مفيش refresh أو فشل التجديد.
  var session = { idToken: "", refreshToken: "", expiresAt: 0 };

  function resetSession() {
    session = { idToken: "", refreshToken: "", expiresAt: 0 };
  }

  async function signIn() {
    var now = Date.now();
    if (session.idToken && now < session.expiresAt - 60000) return session.idToken;

    if (session.refreshToken && now >= session.expiresAt - 60000) {
      try {
        var rres = await tfetch(REFRESH_URL, {
          method: "POST",
          headers: { "Content-Type": "application/x-www-form-urlencoded" },
          body:
            "grant_type=refresh_token&refresh_token=" +
            encodeURIComponent(session.refreshToken),
        });
        if (rres.ok) {
          var rj = await rres.json();
          session.idToken = rj.id_token;
          session.refreshToken = rj.refresh_token;
          session.expiresAt = Date.now() + Number(rj.expires_in || 3600) * 1000;
          return session.idToken;
        }
      } catch (_e) {
        /* بنسقط لتسجيل دخول كامل تحت */
      }
      resetSession();
    }

    var cred = deps.getCred() || {};
    if (!cred.email || !cred.password) {
      throw new Error(
        "محتاجة تحطي بريد وكلمة سر حساب الإدارة في إعدادات أمين الأول."
      );
    }
    var res = await tfetch(SIGNIN_URL, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        email: cred.email,
        password: cred.password,
        returnSecureToken: true,
      }),
    });
    var body = await res.json().catch(function () {
      return {};
    });
    if (!res.ok) {
      var m = (body.error && body.error.message) || "فشل تسجيل الدخول";
      if (m === "INVALID_LOGIN_CREDENTIALS" || m === "INVALID_PASSWORD" || m === "EMAIL_NOT_FOUND")
        throw new Error("بريد أو كلمة سر الإدارة غير صحيحة.");
      throw new Error("فشل تسجيل دخول الإدارة: " + m);
    }
    session.idToken = body.idToken;
    session.refreshToken = body.refreshToken;
    session.expiresAt = Date.now() + Number(body.expiresIn || 3600) * 1000;
    return session.idToken;
  }

  // نداء دالة خلفية قابلة للاستدعاء: POST {data} مع Bearer التوكن، والرد
  // {result} عند النجاح أو {error:{message,status}} عند الفشل (بروتوكول
  // Firebase callable الرسمي).
  async function callFunction(name, data) {
    var token = await signIn();
    var res = await tfetch(FUNCTIONS_BASE + name, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: "Bearer " + token,
      },
      body: JSON.stringify({ data: data || {} }),
    });
    var body = await res.json().catch(function () {
      return {};
    });
    if (!res.ok || body.error) {
      var err = body.error || {};
      // توكن منتهي/مرفوض؟ نصفّر الجلسة عشان النداء الجاي يعمل دخول جديد.
      if (res.status === 401 || err.status === "UNAUTHENTICATED") resetSession();
      throw new Error(err.message || "فشل نداء " + name + " (" + res.status + ")");
    }
    return body.result;
  }

  /* ------- تعريف الأدوات: كل أداة لها نوع (read/write) ودالة المنصة ------- */
  var TOOLS = {
    school_search_users: {
      kind: "read",
      fn: "searchPlatformUsers",
      map: function (i) {
        return { query: String(i.query || "") };
      },
    },
    school_list_portal_accounts: {
      kind: "read",
      fn: "adminListPortalAccounts",
      map: function (i) {
        return { role: i.role };
      },
    },
    school_create_portal_account: {
      kind: "write",
      fn: "adminCreatePortalAccount",
      map: function (i) {
        return {
          civilId: String(i.civilId || ""),
          role: i.role,
          displayName: String(i.displayName || ""),
          linkedId: i.linkedId ? String(i.linkedId) : undefined,
          phone: i.phone ? String(i.phone) : undefined,
          password: i.password ? String(i.password) : undefined,
        };
      },
      confirm: function (i) {
        return (
          "إنشاء حساب " +
          (i.role === "student" ? "طالب" : "ولي أمر") +
          " باسم «" +
          i.displayName +
          "» برقم مدني " +
          i.civilId +
          "؟"
        );
      },
    },
    school_reset_portal_password: {
      kind: "write",
      fn: "adminResetPortalPassword",
      map: function (i) {
        return { civilId: String(i.civilId || ""), password: String(i.password || "") };
      },
      confirm: function (i) {
        return "تصفير كلمة سر الحساب رقم " + i.civilId + " لكلمة جديدة؟";
      },
    },
    school_set_portal_status: {
      kind: "write",
      fn: "adminSetPortalStatus",
      map: function (i) {
        return { civilId: String(i.civilId || ""), status: i.status };
      },
      confirm: function (i) {
        return (
          (i.status === "disabled" ? "تعطيل" : "تفعيل") +
          " الحساب رقم " +
          i.civilId +
          "؟"
        );
      },
    },
    school_rename_portal_account: {
      kind: "write",
      fn: "adminUpdatePortalAccountName",
      map: function (i) {
        return { civilId: String(i.civilId || ""), displayName: String(i.displayName || "") };
      },
      confirm: function (i) {
        return "تغيير اسم الحساب رقم " + i.civilId + " إلى «" + i.displayName + "»؟";
      },
    },
    school_delete_portal_account: {
      kind: "write",
      fn: "adminDeletePortalAccount",
      map: function (i) {
        return { civilId: String(i.civilId || "") };
      },
      confirm: function (i) {
        return "⚠️ حذف الحساب رقم " + i.civilId + " نهائيًا؟ لا يمكن التراجع.";
      },
    },
  };

  // تعريفات الأدوات بصيغة Anthropic — تتحقن في حلقة أدوات أمين.
  var TOOL_DEFS = [
    {
      name: "school_search_users",
      description:
        "ابحث في مستخدمي منصة درة البيان (معلمين/موظفين/أولياء أمور) بالاسم أو الرقم. قراءة فقط.",
      input_schema: {
        type: "object",
        properties: { query: { type: "string", description: "حرفان على الأقل" } },
        required: ["query"],
      },
    },
    {
      name: "school_list_portal_accounts",
      description: "اسرد حسابات بوابة أولياء الأمور أو الطلاب. قراءة فقط.",
      input_schema: {
        type: "object",
        properties: { role: { type: "string", enum: ["parent", "student"] } },
        required: ["role"],
      },
    },
    {
      name: "school_create_portal_account",
      description:
        "أنشئ حساب بوابة جديد (ولي أمر أو طالب). يتطلب موافقة منى قبل التنفيذ.",
      input_schema: {
        type: "object",
        properties: {
          civilId: { type: "string" },
          role: { type: "string", enum: ["parent", "student"] },
          displayName: { type: "string" },
          linkedId: { type: "string", description: "رقم الطالب المرتبط (لولي الأمر)" },
          phone: { type: "string" },
          password: { type: "string", description: "اختياري؛ 6 أحرف فأكثر" },
        },
        required: ["civilId", "role", "displayName"],
      },
    },
    {
      name: "school_reset_portal_password",
      description: "صفّر كلمة سر حساب بوابة. يتطلب موافقة منى.",
      input_schema: {
        type: "object",
        properties: {
          civilId: { type: "string" },
          password: { type: "string", description: "6 أحرف فأكثر" },
        },
        required: ["civilId", "password"],
      },
    },
    {
      name: "school_set_portal_status",
      description: "فعّل أو عطّل حساب بوابة. يتطلب موافقة منى.",
      input_schema: {
        type: "object",
        properties: {
          civilId: { type: "string" },
          status: { type: "string", enum: ["active", "disabled"] },
        },
        required: ["civilId", "status"],
      },
    },
    {
      name: "school_rename_portal_account",
      description: "غيّر الاسم الظاهر لحساب بوابة. يتطلب موافقة منى.",
      input_schema: {
        type: "object",
        properties: { civilId: { type: "string" }, displayName: { type: "string" } },
        required: ["civilId", "displayName"],
      },
    },
    {
      name: "school_delete_portal_account",
      description: "احذف حساب بوابة نهائيًا. يتطلب موافقة منى.",
      input_schema: {
        type: "object",
        properties: { civilId: { type: "string" } },
        required: ["civilId"],
      },
    },
  ];

  function isSchoolTool(name) {
    return Object.prototype.hasOwnProperty.call(TOOLS, name);
  }

  /*
   * تنفيذ أداة مدرسية. القراءة تتنفذ فورًا. الكتابة لازم تعدي على confirm
   * (دالة بترجع boolean أو Promise<boolean>) — لو منى رفضت، الأداة بترجع
   * {cancelled:true} من غير ما تلمس المنصة خالص.
   */
  async function runTool(name, input, confirmFn) {
    var t = TOOLS[name];
    if (!t) return { error: "أداة مدرسية غير معروفة: " + name };
    input = input || {};

    if (t.kind === "write") {
      var summary = t.confirm ? t.confirm(input) : "تنفيذ " + name + "؟";
      var ok = false;
      try {
        ok = confirmFn ? await confirmFn(summary, name, input) : false;
      } catch (_e) {
        ok = false;
      }
      if (!ok) {
        return { cancelled: true, note: "منى لم توافق على: " + summary };
      }
    }

    try {
      var result = await callFunction(t.fn, t.map(input));
      return { ok: true, result: result };
    } catch (e) {
      return { error: String((e && e.message) || e) };
    }
  }

  var SchoolAdmin = {
    configure: configure,
    signIn: signIn,
    callFunction: callFunction,
    isSchoolTool: isSchoolTool,
    runTool: runTool,
    resetSession: resetSession,
    TOOL_DEFS: TOOL_DEFS,
    _TOOLS: TOOLS, // للاختبار
    _PROJECT_ID: PROJECT_ID,
    _FUNCTIONS_BASE: FUNCTIONS_BASE,
    _SIGNIN_URL: SIGNIN_URL,
  };

  if (typeof module !== "undefined" && module.exports) module.exports = SchoolAdmin;
  // globalThis يغطي المتصفح (window === globalThis) وNode ESM معًا.
  if (typeof globalThis !== "undefined") globalThis.SchoolAdmin = SchoolAdmin;
})();
