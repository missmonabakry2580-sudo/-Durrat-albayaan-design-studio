/*
 * قدرة أمين على كود منصة المدرسة (durrat-bayaan-connect على GitHub).
 * أمين يقدر يقرأ كود المنصة ويشخّص عطل برمجي، ويقترح إصلاحه كـ Pull
 * Request مراجَع — مش تعديل مباشر على المنصة الحيّة، عشان لو الإصلاح غلط
 * ما يوقّعش مدرسة فيها أطفال. فتح أي PR (كتابة) بيعدي على موافقة منى.
 *
 * قابلة للحقن (configure) للاختبار في Node وللتشغيل في المتصفح بنفس الكود.
 * مفتاح GitHub سرّ — على الجهاز فقط، عمره ما يتكتب في الريبو.
 */
(function () {
  "use strict";

  var GH = {
    owner: "missmonabakry2580-sudo",
    repo: "durrat-bayaan-connect",
    api: "https://api.github.com",
  };

  var deps = {
    fetch: typeof fetch !== "undefined" ? fetch.bind(globalThis) : null,
    getToken: function () {
      return "";
    },
  };
  function configure(opts) {
    if (opts && opts.fetch) deps.fetch = opts.fetch;
    if (opts && opts.getToken) deps.getToken = opts.getToken;
  }

  function tfetch(url, opts) {
    var ctrl = typeof AbortController !== "undefined" ? new AbortController() : null;
    var timer = ctrl ? setTimeout(function () { ctrl.abort(); }, 25000) : null;
    var o = Object.assign({}, opts || {}, ctrl ? { signal: ctrl.signal } : {});
    return Promise.resolve(deps.fetch(url, o)).then(
      function (r) { if (timer) clearTimeout(timer); return r; },
      function (e) {
        if (timer) clearTimeout(timer);
        if (e && e.name === "AbortError") throw new Error("انتهت مهلة الاتصال بـGitHub — جرّبي تاني.");
        throw e;
      }
    );
  }

  // Base64 آمن مع UTF-8 (العربي) — GitHub Contents API بيطلب المحتوى base64.
  function b64encodeUtf8(str) {
    var bytes = new TextEncoder().encode(str);
    var bin = "";
    for (var i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i]);
    return typeof btoa !== "undefined" ? btoa(bin) : Buffer.from(bin, "binary").toString("base64");
  }
  function b64decodeUtf8(b64) {
    var bin = typeof atob !== "undefined" ? atob(b64) : Buffer.from(b64, "base64").toString("binary");
    var bytes = new Uint8Array(bin.length);
    for (var i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
    return new TextDecoder().decode(bytes);
  }

  async function gh(path, opts) {
    opts = opts || {};
    var token = deps.getToken();
    if (!token) throw new Error("محتاجة تحطي مفتاح GitHub لكود المنصة في الإعدادات الأول.");
    var res = await tfetch(GH.api + path, {
      method: opts.method || "GET",
      headers: Object.assign(
        {
          Authorization: "Bearer " + token,
          Accept: "application/vnd.github+json",
          "X-GitHub-Api-Version": "2022-11-28",
        },
        opts.body ? { "Content-Type": "application/json" } : {}
      ),
      body: opts.body ? JSON.stringify(opts.body) : undefined,
    });
    var body = await res.json().catch(function () {
      return {};
    });
    if (!res.ok) {
      throw new Error("GitHub: " + ((body && body.message) || res.status));
    }
    return body;
  }

  async function defaultBranch() {
    var repo = await gh("/repos/" + GH.owner + "/" + GH.repo);
    return repo.default_branch || "main";
  }

  async function listFiles(path) {
    path = (path || "").replace(/^\/+/, "");
    var body = await gh("/repos/" + GH.owner + "/" + GH.repo + "/contents/" + path);
    if (Array.isArray(body)) {
      return body.map(function (e) {
        return { name: e.name, path: e.path, type: e.type };
      });
    }
    return [{ name: body.name, path: body.path, type: body.type }];
  }

  async function readFile(path) {
    path = (path || "").replace(/^\/+/, "");
    var body = await gh("/repos/" + GH.owner + "/" + GH.repo + "/contents/" + encodeURIComponent(path).replace(/%2F/g, "/"));
    if (body.type !== "file" || typeof body.content !== "string") {
      throw new Error("ليس ملفًا نصيًا: " + path);
    }
    return { path: body.path, sha: body.sha, content: b64decodeUtf8(body.content.replace(/\n/g, "")) };
  }

  // يقترح إصلاحًا: يفرّع من الفرع الافتراضي، يكتب الملف على الفرع الجديد،
  // ويفتح PR. لا يلمس الفرع الحيّ إطلاقًا.
  async function proposeFix(input) {
    var path = String(input.path || "").replace(/^\/+/, "");
    if (!path || input.content == null) throw new Error("لازم مسار الملف ومحتواه الجديد.");
    var base = await defaultBranch();
    var baseRef = await gh("/repos/" + GH.owner + "/" + GH.repo + "/git/ref/heads/" + encodeURIComponent(base));
    var branch = "amin-fix-" + Date.now();
    await gh("/repos/" + GH.owner + "/" + GH.repo + "/git/refs", {
      method: "POST",
      body: { ref: "refs/heads/" + branch, sha: baseRef.object.sha },
    });
    // sha الملف الحالي (لو موجود) مطلوب للتحديث.
    var sha;
    try {
      var existing = await readFile(path);
      sha = existing.sha;
    } catch (_e) {
      sha = undefined; // ملف جديد
    }
    await gh(
      "/repos/" + GH.owner + "/" + GH.repo + "/contents/" + encodeURIComponent(path).replace(/%2F/g, "/"),
      {
        method: "PUT",
        body: {
          message: String(input.message || ("Amin: fix " + path)),
          content: b64encodeUtf8(String(input.content)),
          branch: branch,
          sha: sha,
        },
      }
    );
    var pr = await gh("/repos/" + GH.owner + "/" + GH.repo + "/pulls", {
      method: "POST",
      body: {
        title: String(input.pr_title || ("Amin: إصلاح " + path)),
        head: branch,
        base: base,
        body: String(input.pr_body || "إصلاح مقترح من أمين — للمراجعة قبل الدمج."),
      },
    });
    return { pr_number: pr.number, pr_url: pr.html_url, branch: branch };
  }

  /* ---------------------------- أدوات أمين ----------------------------- */
  var TOOLS = {
    code_list_files: { kind: "read" },
    code_read_file: { kind: "read" },
    code_propose_fix: { kind: "write" },
  };

  var TOOL_DEFS = [
    {
      name: "code_list_files",
      description:
        "اسرد ملفات ومجلدات كود منصة المدرسة عند مسار معيّن (فارغ = الجذر). للتشخيص. قراءة.",
      input_schema: {
        type: "object",
        properties: { path: { type: "string", description: "مسار داخل المستودع، فارغ للجذر" } },
      },
    },
    {
      name: "code_read_file",
      description: "اقرأ محتوى ملف من كود منصة المدرسة لتشخيص عطل برمجي. قراءة.",
      input_schema: {
        type: "object",
        properties: { path: { type: "string" } },
        required: ["path"],
      },
    },
    {
      name: "code_propose_fix",
      description:
        "اقترح إصلاحًا لملف في كود المنصة كـ Pull Request مراجَع (لا ينشر مباشرة). content هو المحتوى الكامل الجديد للملف. يتطلب موافقة منى دائمًا.",
      input_schema: {
        type: "object",
        properties: {
          path: { type: "string" },
          content: { type: "string", description: "المحتوى الكامل الجديد للملف" },
          message: { type: "string", description: "رسالة الـ commit" },
          pr_title: { type: "string" },
          pr_body: { type: "string", description: "شرح العطل والإصلاح" },
        },
        required: ["path", "content"],
      },
    },
  ];

  function isCodeTool(name) {
    return Object.prototype.hasOwnProperty.call(TOOLS, name);
  }

  async function runTool(name, input, confirmFn) {
    if (!isCodeTool(name)) return { error: "أداة كود غير معروفة: " + name };
    input = input || {};

    if (TOOLS[name].kind === "write") {
      var summary =
        "فتح Pull Request بإصلاح لملف «" + input.path + "» في كود المنصة (للمراجعة قبل النشر)؟";
      var ok = false;
      try {
        ok = confirmFn ? await confirmFn(summary, name, input) : false;
      } catch (_e) {
        ok = false;
      }
      if (!ok) return { cancelled: true, note: "منى لم توافق على: " + summary };
    }

    try {
      switch (name) {
        case "code_list_files":
          return { ok: true, entries: await listFiles(input.path) };
        case "code_read_file": {
          var f = await readFile(input.path);
          return { ok: true, path: f.path, content: f.content };
        }
        case "code_propose_fix":
          return { ok: true, result: await proposeFix(input) };
        default:
          return { error: "أداة غير مدعومة: " + name };
      }
    } catch (e) {
      return { error: String((e && e.message) || e) };
    }
  }

  var SchoolCode = {
    configure: configure,
    isCodeTool: isCodeTool,
    runTool: runTool,
    TOOL_DEFS: TOOL_DEFS,
    _TOOLS: TOOLS,
    _b64: { enc: b64encodeUtf8, dec: b64decodeUtf8 },
    _GH: GH,
  };

  if (typeof module !== "undefined" && module.exports) module.exports = SchoolCode;
  if (typeof globalThis !== "undefined") globalThis.SchoolCode = SchoolCode;
})();
