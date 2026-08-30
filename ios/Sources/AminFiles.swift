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
        case "photos_delete":          photosDelete(args, replyHandler)
        case "photos_create_album":    photosCreateAlbum(args, replyHandler)
        case "photos_organize_by_month": photosOrganizeByMonth(replyHandler)
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

    private func photosSummary(_ reply: @escaping (Any?, String?) -> Void) {
        ensurePhotoAccess { ok in
            guard ok else { reply(nil, "مفيش إذن للصور."); return }
            let images = PHAsset.fetchAssets(with: .image, options: nil)
            let videos = PHAsset.fetchAssets(with: .video, options: nil)
            var byMonth: [String: Int] = [:]
            let fmt = DateFormatter()
            fmt.dateFormat = "yyyy-MM"
            images.enumerateObjects { asset, _, _ in
                if let d = asset.creationDate {
                    let k = fmt.string(from: d)
                    byMonth[k, default: 0] += 1
                }
            }
            reply([
                "photos": images.count,
                "videos": videos.count,
                "byMonth": byMonth
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
    private func photosOrganizeByMonth(_ reply: @escaping (Any?, String?) -> Void) {
        ensurePhotoAccess { ok in
            guard ok else { reply(nil, "مفيش إذن للصور."); return }
            let images = PHAsset.fetchAssets(with: .image, options: nil)
            let fmt = DateFormatter()
            fmt.dateFormat = "yyyy-MM"
            var byMonth: [String: [String]] = [:]
            images.enumerateObjects { asset, _, _ in
                if let d = asset.creationDate {
                    byMonth[fmt.string(from: d), default: []].append(asset.localIdentifier)
                }
            }
            let months = byMonth.keys.sorted()
            self.createMonthAlbums(months: months, byMonth: byMonth, index: 0, created: 0, reply: reply)
        }
    }

    // بننشئ ألبومات الشهور واحد ورا التاني (تسلسل عشان نتفادى تعارض
    // تعديلات المكتبة المتوازية).
    private func createMonthAlbums(
        months: [String], byMonth: [String: [String]],
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
            let req = PHAssetCollectionChangeRequest.creationRequestForAssetCollection(withTitle: "أمين — \(month)")
            placeholder = req.placeholderForCreatedAssetCollection
        } completionHandler: { success, _ in
            guard success, let ph = placeholder else {
                self.createMonthAlbums(months: months, byMonth: byMonth, index: index + 1, created: created, reply: reply)
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
                self.createMonthAlbums(months: months, byMonth: byMonth, index: index + 1, created: created + 1, reply: reply)
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
