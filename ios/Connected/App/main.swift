import SwiftUI

#if canImport(UIKit)
import UIKit
#endif

struct ConnectedIOSApp: App {
    @StateObject private var model = ConnectedAppModel()
    @Environment(\.scenePhase) private var scenePhase

    var body: some Scene {
        WindowGroup {
            ConnectedRootView()
                .environmentObject(model)
                .preferredColorScheme(preferredColorScheme(for: model.themeMode))
                .task {
                    model.initializeIfNeeded()
                }
                .onOpenURL { url in
                    model.handleIncomingShareURL(url)
                }
                .onChangeCompat(of: scenePhase) { newPhase in
                    switch newPhase {
                    case .active:
                        model.handleAppBecameActive()
                    case .background:
                        model.handleAppEnteredBackground()
                    default:
                        break
                    }
                }
        }
    }

    private func preferredColorScheme(for mode: ConnectedAppModel.ThemeMode) -> ColorScheme? {
        switch mode {
        case .system:
            return nil
        case .light:
            return .light
        case .dark:
            return .dark
        }
    }
}

ConnectedIOSApp.main()
