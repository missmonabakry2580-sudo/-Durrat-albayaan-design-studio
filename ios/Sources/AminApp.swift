// أمين — نسخة الآيفون الأصلية. See project.yml's header for why this
// exists; every hard-won lesson from the web version's live debugging with
// Mona (2026-08-29) is applied here from line one: silence-based
// finalization for the mic (never trust the platform's end-of-speech
// callback alone), speaking never blocks the pipeline, every network call
// has a timeout, and failures surface as visible text — never silence.
import SwiftUI

@main
struct AminApp: App {
    var body: some Scene {
        WindowGroup {
            ContentView()
                .preferredColorScheme(.dark)
                .environment(\.layoutDirection, .rightToLeft)
        }
    }
}
