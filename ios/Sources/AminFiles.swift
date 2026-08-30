// جسر تنظيم ملفات الجوال — الشيء الوحيد اللي التطبيق الأصلي بيقدر يعمله
// وتطبيق الويب مستحيل يعمله بسبب صندوق iOS الرملي (sandbox):
//
//   ١) الصور (PhotoKit): جرد، إيجاد المكرر، حذف بتأكيد النظام، إنشاء
//      ألبومات، ترتيب بالشهر.
//   ٢) مجلد واحد تختاره منى (UIDocumentPickerViewController + إذن محدود
//      النطاق): سرد، إيجاد/حذف المكرر بالبصمة، ترتيب حسب النوع.
//
// أمين في الويب بيندهله عن طريق window.webkit.messageHandlers.aminFiles
// (WKScriptMessageHandlerWithReply) وبيرجع Promise. كل عملية تدميرية
// بتتأكد من منى الأول — إما من طبقة الويب أو من حوار النظام نفسه.
import Foundation
import WebKit
import Photos
import UIKit
import CryptoKit
import UniformTypeIdentifiers

// مش @MainActor: WebKit بينده الجسر على الـ main thread، والمعالجات
// (delegate + reply) كلها بترجع على main — فالتزام بروتوكول WebKit
// (اللي مش isolated) بيفضل نظيف من غير تحذيرات عزل.
final class AminFiles: NSObject, WKScriptMessageHandlerWithReply {
    // المجلد اللي اختارته منى آخر مرة (بإذن محدود النطاق). بنحتفظ بيه عشان
    // العمليات المتتالية (سرد ثم ترتيب) تشتغل من غير ما تختار كل مرة.
    private var pickedFolder: URL?
    // ننتظر اختيار المجلد بشكل غير متزامن — بنخزن رد الـ Promise لحد ما
    // منى تختار أو تلغي.
    private var pendingPickReply: ((Any?, String?) -> Void)?

    // MARK: - نقطة الدخول من الويب

    func userContentController(
        _ userContentController: WKUserContentController,
        didReceive message: WKScriptMessage,
        replyHandler: @escaping (Any?, String?) -> Void
    ) {
        guard let body = message.body as? [String: Any],
              let action = body["action"] as? String else {
            replyHandler(nil, "طلب غير صالح")
            return
        }
        let args = body["args"] as? [String: Any] ?? [:]

        switch action {
        case "photos_authorize":       photosAuthorize(replyHandler)
        case "photos_summary":         photosSummary(replyHandler)
        case "photos_find_duplicates": photosFindDuplicates(replyHandler)
        case "photos_find_similar":    photosFindSimilar(args, replyHandler)
        case "photos_delete":          photosDelete(args, replyHandler)
        case "photos_create_album":    photosCreateAlbum(args, replyHandler)
        case "photos_organize_by_month": photosOrganizeByMonth(args, replyHandler)
        case "files_pick_folder":      filesPickFolder(replyHandler)
        case "files_list":             filesList(replyHandler)
        case "files_find_duplicates":  filesFindDuplicates(replyHandler)
        case "files_organize_by_type": filesOrganizeByType(args, replyHandler)
        default:
            replyHandler(nil, "أمر غير معروف: \(action)")
        }
    }

    // MARK: - الصور (PhotoKit)

    private func ensurePhotoAccess(_ then: @escaping (Bool) -> Void) {
        let status = PHPhotoLibrary.authorizationStatus(for: .readWrite)
        switch status {
        case .authorized, .limited:
            then(true)
        case .notDetermined:
            PHPhotoLibrary.requestAuthorization(for: .readWrite) { newStatus in
                DispatchQueue.main.async {
                    then(newStatus == .authorized || newStatus == .limited)
                }
            }
        default:
            then(false)
        }
    }

    private func photosAuthorize(_ reply: @escaping (Any?, String?) -> Void) {
        ensurePhotoAccess { ok in
            if ok { reply(["granted": true], nil) }
            else { reply(nil, "لازم تسمحي لأمين بالوصول للصور من الإعدادات.") }
        }
    }

    // تاريخ إضافة الصورة لمكتبة الجهاز ده (مش تاريخ التصوير الأصلي في EXIF).
    // مهم للصور اللي جت من واتساب/تليجرام/نسخة احتياطية: تاريخ تصويرها قديم
    // بس هي اتضافت للتليفون ده حديثًا. PhotoKit مبيعرضش addedDate في الهيدر
    // العام، فبنقراه بحذر عبر KVC مع حماية ضد الانهيار، ولو مش متاح
    // بنرجع لتاريخ التصوير. (لو أبل رفضت الرمز ده يومًا، ده المكان الوحيد
    // اللي يتشال منه — والباقي بيفضل شغال بتاريخ التصوير.)
    private func assetAddedDate(_ asset: PHAsset) -> Date? {
        if asset.responds(to: NSSelectorFromString("addedDate")),
           let d = asset.value(forKey: "addedDate") as? Date {
            return d
        }
        return nil
    }

    private func photosSummary(_ reply: @escaping (Any?, String?) -> Void) {
        ensurePhotoAccess { ok in
            guard ok else { reply(nil, "مفيش إذن للصور."); return }
            let images = PHAsset.fetchAssets(with: .image, options: nil)
            let videos = PHAsset.fetchAssets(with: .video, options: nil)
            var byMonthCaptured: [String: Int] = [:]  // تاريخ التصوير (EXIF)
            var byMonthAdded: [String: Int] = [:]      // تاريخ الإضافة للجهاز
            var addedAvailable = false
            let fmt = DateFormatter()
            fmt.dateFormat = "yyyy-MM"
            images.enumerateObjects { asset, _, _ in
                if let d = asset.creationDate {
                    byMonthCaptured[fmt.string(from: d), default: 0] += 1
                }
                if let a = self.assetAddedDate(asset) {
                    addedAvailable = true
                    byMonthAdded[fmt.string(from: a), default: 0] += 1
                }
            }
            reply([
                "photos": images.count,
                "videos": videos.count,
                // بالاسمين الواضحين عشان ميحصلش لبس بين التاريخين تاني.
                "byMonthCaptured": byMonthCaptured,
                "byMonthAdded": byMonthAdded,
                "addedDateAvailable": addedAvailable,
                // اسم قديم للتوافق: نفس تاريخ التصوير.
                "byMonth": byMonthCaptured
            ], nil)
        }
    }

    // بندوّر على الصور المكررة: نفس تاريخ الالتقاط (للثانية) ونفس الأبعاد.
    // ده كشف عملي وسريع للنسخ المتكررة من غير ما نحمّل كل صورة للذاكرة.
    private func photosFindDuplicates(_ reply: @escaping (Any?, String?) -> Void) {
        ensurePhotoAccess { ok in
            guard ok else { reply(nil, "مفيش إذن للصور."); return }
            let images = PHAsset.fetchAssets(with: .image, options: nil)
            var groups: [String: [[String: Any]]] = [:]
            images.enumerateObjects { asset, _, _ in
                let t = asset.creationDate?.timeIntervalSince1970 ?? 0
                let key = "\(Int(t))_\(asset.pixelWidth)x\(asset.pixelHeight)"
                groups[key, default: []].append([
                    "id": asset.localIdentifier,
                    "width": asset.pixelWidth,
                    "height": asset.pixelHeight,
                    "date": t
                ])
            }
            // بس المجموعات اللي فيها أكتر من صورة = مكررة فعلًا.
            let dups = groups.values.filter { $0.count > 1 }
            let totalExtra = dups.reduce(0) { $0 + ($1.count - 1) }
            reply([
                "groups": Array(dups),
                "duplicateGroups": dups.count,
                "extraCopies": totalExtra
            ], nil)
        }
    }

    // بصمة إدراكية (dHash 9×8 → ٦٤ بت) من بكسلات الصورة نفسها. بتقارن
    // المحتوى البصري مش البيانات الوصفية — فبتكشف نفس الصورة حتى لو
    // اتصغّرت أو اتقصّت أو اتصورت في وقت مختلف. الفرق بين بصمتين
    // (Hamming distance) بيتحول لنسبة تطابق %.
    private func dHash(_ cgImage: CGImage) -> UInt64? {
        let w = 9, h = 8
        var pixels = [UInt8](repeating: 0, count: w * h)
        let gray = CGColorSpaceCreateDeviceGray()
        guard let ctx = CGContext(
            data: &pixels, width: w, height: h, bitsPerComponent: 8,
            bytesPerRow: w, space: gray,
            bitmapInfo: CGImageAlphaInfo.none.rawValue) else { return nil }
        ctx.interpolationQuality = .low
        ctx.draw(cgImage, in: CGRect(x: 0, y: 0, width: w, height: h))
        var hash: UInt64 = 0
        var bit = 0
        for row in 0..<h {
            for col in 0..<(w - 1) {
                if pixels[row * w + col] > pixels[row * w + col + 1] {
                    hash |= (UInt64(1) << UInt64(bit))
                }
                bit += 1
            }
        }
        return hash
    }

    // كشف الصور المتشابهة بصريًا مع نسبة تطابق لكل مجموعة. القراءة بس —
    // مفيش حذف. بيرجّع النسبة عشان منى تتأكد ١٠٠٪ قبل أي حذف.
    // بيشتغل على thumbnail محلي (fastFormat, من غير إنترنت) فسريع نسبيًا
    // وبيشتغل حتى لو الأصل متخزّن على iCloud.
    private func photosFindSimilar(_ args: [String: Any], _ reply: @escaping (Any?, String?) -> Void) {
        // العتبة الافتراضية ٩٠٪ — عالية عشان الأمان. منى تقدر ترفعها.
        let minSim = max(50.0, min(100.0, (args["minSimilarity"] as? Double) ?? 90.0))
        ensurePhotoAccess { ok in
            guard ok else { reply(nil, "مفيش إذن للصور."); return }
            // الشغل تقيل (قراءة كل الصور) — على خيط خلفي عشان الواجهة متجمّدش.
            DispatchQueue.global(qos: .userInitiated).async {
                let images = PHAsset.fetchAssets(with: .image, options: nil)
                let manager = PHImageManager.default()
                let opts = PHImageRequestOptions()
                opts.isSynchronous = true          // تسلسلي على الخيط الخلفي
                opts.deliveryMode = .fastFormat     // thumbnail يكفي للبصمة
                opts.resizeMode = .fast
                opts.isNetworkAccessAllowed = false // محلي بس — أسرع وبدون بيانات
                let target = CGSize(width: 32, height: 32)

                var ids: [String] = []
                var hashes: [UInt64] = []
                images.enumerateObjects { asset, _, _ in
                    manager.requestImage(
                        for: asset, targetSize: target,
                        contentMode: .aspectFill, options: opts
                    ) { image, _ in
                        if let cg = image?.cgImage, let h = self.dHash(cg) {
                            ids.append(asset.localIdentifier)
                            hashes.append(h)
                        }
                    }
                }

                // تجميع جشع: كل صورة مش متجمّعة بتلمّ اللي يتشابه معاها.
                var used = [Bool](repeating: false, count: ids.count)
                var groups: [[String: Any]] = []
                var extra = 0
                for i in 0..<ids.count {
                    if used[i] { continue }
                    var members: [[String: Any]] = [["id": ids[i], "similarity": 100]]
                    var minGroupSim = 100.0
                    for j in (i + 1)..<ids.count {
                        if used[j] { continue }
                        let ham = (hashes[i] ^ hashes[j]).nonzeroBitCount
                        let sim = Double(64 - ham) / 64.0 * 100.0
                        if sim >= minSim {
                            used[j] = true
                            members.append(["id": ids[j], "similarity": Int(sim.rounded())])
                            minGroupSim = Swift.min(minGroupSim, sim)
                        }
                    }
                    if members.count > 1 {
                        used[i] = true
                        extra += members.count - 1
                        groups.append([
                            "members": members,
                            "count": members.count,
                            "minSimilarity": Int(minGroupSim.rounded())
                        ])
                    }
                }
                // نرتّب الأقوى تشابهًا الأول، ونحد الحجم عشان الرد ميكبرش.
                groups.sort { (($0["minSimilarity"] as? Int) ?? 0) > (($1["minSimilarity"] as? Int) ?? 0) }
                let capped = Array(groups.prefix(200))

                DispatchQueue.main.async {
                    reply([
                        "scanned": ids.count,
                        "duplicateGroups": groups.count,
                        "extraCopies": extra,
                        "minSimilarityUsed": Int(minSim.rounded()),
                        "groups": capped,
                        "note": groups.count > capped.count
                            ? "معروض أقوى \(capped.count) مجموعة من إجمالي \(groups.count)."
                            : ""
                    ], nil)
                }
            }
        }
    }

    // حذف صور بمعرّفاتها. النظام نفسه بيعرض حوار تأكيد قبل الحذف الفعلي —
    // فمنى دايمًا بتوافق بإيدها، مفيش حذف صامت.
    private func photosDelete(_ args: [String: Any], _ reply: @escaping (Any?, String?) -> Void) {
        guard let ids = args["ids"] as? [String], !ids.isEmpty else {
            reply(nil, "مفيش صور محددة للحذف."); return
        }
        ensurePhotoAccess { ok in
            guard ok else { reply(nil, "مفيش إذن للصور."); return }
            let assets = PHAsset.fetchAssets(withLocalIdentifiers: ids, options: nil)
            PHPhotoLibrary.shared().performChanges {
                PHAssetChangeRequest.deleteAssets(assets)
            } completionHandler: { success, error in
                DispatchQueue.main.async {
                    if success { reply(["deleted": assets.count], nil) }
                    else { reply(nil, error?.localizedDescription ?? "الحذف اتلغى.") }
                }
            }
        }
    }

    private func photosCreateAlbum(_ args: [String: Any], _ reply: @escaping (Any?, String?) -> Void) {
        guard let name = args["name"] as? String, !name.isEmpty else {
            reply(nil, "لازم اسم للألبوم."); return
        }
        let ids = args["ids"] as? [String] ?? []
        ensurePhotoAccess { ok in
            guard ok else { reply(nil, "مفيش إذن للصور."); return }
            var placeholder: PHObjectPlaceholder?
            PHPhotoLibrary.shared().performChanges {
                let req = PHAssetCollectionChangeRequest.creationRequestForAssetCollection(withTitle: name)
                placeholder = req.placeholderForCreatedAssetCollection
            } completionHandler: { success, error in
                guard success, let ph = placeholder else {
                    DispatchQueue.main.async { reply(nil, error?.localizedDescription ?? "فشل إنشاء الألبوم.") }
                    return
                }
                if ids.isEmpty {
                    DispatchQueue.main.async { reply(["album": name, "added": 0], nil) }
                    return
                }
                let coll = PHAssetCollection.fetchAssetCollections(
                    withLocalIdentifiers: [ph.localIdentifier], options: nil).firstObject
                let assets = PHAsset.fetchAssets(withLocalIdentifiers: ids, options: nil)
                PHPhotoLibrary.shared().performChanges {
                    if let coll = coll,
                       let addReq = PHAssetCollectionChangeRequest(for: coll) {
                        addReq.addAssets(assets)
                    }
                } completionHandler: { ok2, err2 in
                    DispatchQueue.main.async {
                        if ok2 { reply(["album": name, "added": assets.count], nil) }
                        else { reply(nil, err2?.localizedDescription ?? "فشل إضافة الصور للألبوم.") }
                    }
                }
            }
        }
    }

    // بيعمل ألبوم لكل شهر (yyyy-MM) وبيحط صور الشهر جواه — ترتيب من غير حذف.
    // dateField بيحدد بأي تاريخ نصنّف: "captured" (تاريخ التصوير الأصلي) أو
    // "added" (تاريخ إضافة الصورة لمكتبة الجهاز ده). أمين لازم يسأل منى
    // الأول — عشان الفرق ما يتكررش. اسم الألبوم بيوضّح التاريخ المستخدم.
    private func photosOrganizeByMonth(_ args: [String: Any], _ reply: @escaping (Any?, String?) -> Void) {
        let useAdded = (args["dateField"] as? String) == "added"
        ensurePhotoAccess { ok in
            guard ok else { reply(nil, "مفيش إذن للصور."); return }
            // لو منى طلبت التصنيف بتاريخ الإضافة بس الجهاز مش بيوفّره، نوقف
            // ونقول بصراحة بدل ما نصنّف بالتاريخ الغلط بصمت.
            if useAdded {
                let probe = PHAsset.fetchAssets(with: .image, options: nil).firstObject
                if probe != nil && self.assetAddedDate(probe!) == nil {
                    reply(nil, "تاريخ الإضافة للجهاز مش متاح على النظام ده — أقدر أصنّف بتاريخ التصوير الأصلي بس.")
                    return
                }
            }
            let images = PHAsset.fetchAssets(with: .image, options: nil)
            let fmt = DateFormatter()
            fmt.dateFormat = "yyyy-MM"
            var byMonth: [String: [String]] = [:]
            images.enumerateObjects { asset, _, _ in
                let date = useAdded ? self.assetAddedDate(asset) : asset.creationDate
                if let d = date {
                    byMonth[fmt.string(from: d), default: []].append(asset.localIdentifier)
                }
            }
            let months = byMonth.keys.sorted()
            let prefix = useAdded ? "أمين (أُضيفت)" : "أمين"
            self.createMonthAlbums(months: months, prefix: prefix, byMonth: byMonth, index: 0, created: 0, reply: reply)
        }
    }

    // بننشئ ألبومات الشهور واحد ورا التاني (تسلسل عشان نتفادى تعارض
    // تعديلات المكتبة المتوازية).
    private func createMonthAlbums(
        months: [String], prefix: String, byMonth: [String: [String]],
        index: Int, created: Int, reply: @escaping (Any?, String?) -> Void
    ) {
        if index >= months.count {
            reply(["albumsCreated": created, "months": months], nil)
            return
        }
        let month = months[index]
        let ids = byMonth[month] ?? []
        var placeholder: PHObjectPlaceholder?
        PHPhotoLibrary.shared().performChanges {
            let req = PHAssetCollectionChangeRequest.creationRequestForAssetCollection(withTitle: "\(prefix) — \(month)")
            placeholder = req.placeholderForCreatedAssetCollection
        } completionHandler: { success, _ in
            guard success, let ph = placeholder else {
                self.createMonthAlbums(months: months, prefix: prefix, byMonth: byMonth, index: index + 1, created: created, reply: reply)
                return
            }
            let coll = PHAssetCollection.fetchAssetCollections(
                withLocalIdentifiers: [ph.localIdentifier], options: nil).firstObject
            let assets = PHAsset.fetchAssets(withLocalIdentifiers: ids, options: nil)
            PHPhotoLibrary.shared().performChanges {
                if let coll = coll,
                   let addReq = PHAssetCollectionChangeRequest(for: coll) {
                    addReq.addAssets(assets)
                }
            } completionHandler: { _, _ in
                self.createMonthAlbums(months: months, prefix: prefix, byMonth: byMonth, index: index + 1, created: created + 1, reply: reply)
            }
        }
    }

    // MARK: - الملفات (مجلد تختاره منى)

    private func topViewController() -> UIViewController? {
        let scenes = UIApplication.shared.connectedScenes
        let windowScene = scenes.first { $0.activationState == .foregroundActive } as? UIWindowScene
        var top = windowScene?.windows.first { $0.isKeyWindow }?.rootViewController
            ?? windowScene?.windows.first?.rootViewController
        while let presented = top?.presentedViewController { top = presented }
        return top
    }

    private func filesPickFolder(_ reply: @escaping (Any?, String?) -> Void) {
        guard pendingPickReply == nil else {
            reply(nil, "في اختيار مجلد شغّال بالفعل."); return
        }
        guard let vc = topViewController() else {
            reply(nil, "مش قادر أفتح نافذة الاختيار."); return
        }
        pendingPickReply = reply
        let picker = UIDocumentPickerViewController(forOpeningContentTypes: [.folder])
        picker.allowsMultipleSelection = false
        picker.delegate = self
        vc.present(picker, animated: true)
    }

    // بيقرأ محتويات المجلد المختار مع النوع والحجم.
    private func filesList(_ reply: @escaping (Any?, String?) -> Void) {
        guard let folder = pickedFolder else {
            reply(nil, "اختاري مجلد الأول."); return
        }
        let scoped = folder.startAccessingSecurityScopedResource()
        defer { if scoped { folder.stopAccessingSecurityScopedResource() } }
        do {
            let items = try FileManager.default.contentsOfDirectory(
                at: folder,
                includingPropertiesForKeys: [.fileSizeKey, .isDirectoryKey],
                options: [.skipsHiddenFiles])
            var files: [[String: Any]] = []
            var byType: [String: Int] = [:]
            for url in items {
                let vals = try? url.resourceValues(forKeys: [.fileSizeKey, .isDirectoryKey])
                let isDir = vals?.isDirectory ?? false
                let ext = isDir ? "مجلد" : (url.pathExtension.isEmpty ? "بدون" : url.pathExtension.lowercased())
                byType[ext, default: 0] += 1
                files.append([
                    "name": url.lastPathComponent,
                    "ext": ext,
                    "isDir": isDir,
                    "size": vals?.fileSize ?? 0
                ])
            }
            reply(["folder": folder.lastPathComponent, "count": files.count,
                   "byType": byType, "files": files], nil)
        } catch {
            reply(nil, "مش قادر أقرا المجلد: \(error.localizedDescription)")
        }
    }

    // بصمة محتوى (SHA-256) لكل ملف عشان نكتشف المكرر الحقيقي حتى لو الاسم مختلف.
    private func filesFindDuplicates(_ reply: @escaping (Any?, String?) -> Void) {
        guard let folder = pickedFolder else {
            reply(nil, "اختاري مجلد الأول."); return
        }
        let scoped = folder.startAccessingSecurityScopedResource()
        defer { if scoped { folder.stopAccessingSecurityScopedResource() } }
        do {
            let items = try FileManager.default.contentsOfDirectory(
                at: folder, includingPropertiesForKeys: [.isDirectoryKey],
                options: [.skipsHiddenFiles])
            var byHash: [String: [String]] = [:]
            for url in items {
                let isDir = (try? url.resourceValues(forKeys: [.isDirectoryKey]))?.isDirectory ?? false
                if isDir { continue }
                guard let data = try? Data(contentsOf: url) else { continue }
                let hash = SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
                byHash[hash, default: []].append(url.lastPathComponent)
            }
            let dups = byHash.values.filter { $0.count > 1 }
            let extra = dups.reduce(0) { $0 + ($1.count - 1) }
            reply(["duplicateGroups": dups.count, "extraCopies": extra,
                   "groups": Array(dups)], nil)
        } catch {
            reply(nil, "مش قادر أفحص المجلد: \(error.localizedDescription)")
        }
    }

    // بينقل كل ملف لمجلد فرعي حسب نوعه (صور/PDF/مستندات…). ده ترتيب داخل
    // نفس المجلد المختار — مفيش حذف. منى بتوافق من طبقة الويب قبل التنفيذ.
    private func filesOrganizeByType(_ args: [String: Any], _ reply: @escaping (Any?, String?) -> Void) {
        guard let folder = pickedFolder else {
            reply(nil, "اختاري مجلد الأول."); return
        }
        let scoped = folder.startAccessingSecurityScopedResource()
        defer { if scoped { folder.stopAccessingSecurityScopedResource() } }
        let fm = FileManager.default
        do {
            let items = try fm.contentsOfDirectory(
                at: folder, includingPropertiesForKeys: [.isDirectoryKey],
                options: [.skipsHiddenFiles])
            var moved = 0
            var buckets: [String: Int] = [:]
            for url in items {
                let isDir = (try? url.resourceValues(forKeys: [.isDirectoryKey]))?.isDirectory ?? false
                if isDir { continue }
                let bucket = self.bucketName(for: url.pathExtension.lowercased())
                let dir = folder.appendingPathComponent(bucket, isDirectory: true)
                if !fm.fileExists(atPath: dir.path) {
                    try? fm.createDirectory(at: dir, withIntermediateDirectories: true)
                }
                var dest = dir.appendingPathComponent(url.lastPathComponent)
                // لو في ملف بنفس الاسم، نضيف رقم عشان ما نكتبش فوقه.
                var n = 1
                while fm.fileExists(atPath: dest.path) {
                    let base = url.deletingPathExtension().lastPathComponent
                    let ext = url.pathExtension
                    let newName = ext.isEmpty ? "\(base) (\(n))" : "\(base) (\(n)).\(ext)"
                    dest = dir.appendingPathComponent(newName)
                    n += 1
                }
                do {
                    try fm.moveItem(at: url, to: dest)
                    moved += 1
                    buckets[bucket, default: 0] += 1
                } catch { /* نتخطى الملف اللي مش عارفين ننقله */ }
            }
            reply(["moved": moved, "folders": buckets], nil)
        } catch {
            reply(nil, "مش قادر أرتب المجلد: \(error.localizedDescription)")
        }
    }

    private func bucketName(for ext: String) -> String {
        switch ext {
        case "jpg", "jpeg", "png", "heic", "gif", "webp", "bmp", "tiff":
            return "صور"
        case "mp4", "mov", "m4v", "avi", "mkv":
            return "فيديو"
        case "mp3", "m4a", "wav", "aac", "flac":
            return "صوت"
        case "pdf":
            return "PDF"
        case "doc", "docx", "pages", "txt", "rtf", "md":
            return "مستندات"
        case "xls", "xlsx", "numbers", "csv":
            return "جداول"
        case "ppt", "pptx", "key":
            return "عروض"
        case "zip", "rar", "7z", "tar", "gz":
            return "مضغوط"
        default:
            return "أخرى"
        }
    }
}

// MARK: - نتيجة اختيار المجلد

extension AminFiles: UIDocumentPickerDelegate {
    func documentPicker(_ controller: UIDocumentPickerViewController,
                        didPickDocumentsAt urls: [URL]) {
        let reply = pendingPickReply
        pendingPickReply = nil
        guard let url = urls.first else {
            reply?(nil, "مفيش مجلد اتختار."); return
        }
        pickedFolder = url
        reply?(["folder": url.lastPathComponent, "path": url.path], nil)
    }

    func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
        let reply = pendingPickReply
        pendingPickReply = nil
        reply?(nil, "اختيار المجلد اتلغى.")
    }
}
