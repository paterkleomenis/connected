import SwiftUI

/// Unified color theme matching Android's Material 3 palette + modern glass aesthetics
enum GlassTheme {
    // MARK: - Dark Mode Colors (Matching Android)
    static let darkBackground = Color(red: 0, green: 0, blue: 0)                // #000000
    static let darkSurface = Color(red: 0, green: 0, blue: 0)                   // #000000
    static let darkSurfaceVariant = Color(red: 0.11, green: 0.11, blue: 0.118)  // #1C1C1E
    static let darkPrimary = Color.white                                          // #FFFFFF
    static let darkOnPrimary = Color.black                                        // #000000
    static let darkOnSurface = Color.white                                        // #FFFFFF
    static let darkOnSurfaceVariant = Color(red: 0.82, green: 0.82, blue: 0.84) // #D1D1D6
    static let darkOutline = Color(red: 0.557, green: 0.557, blue: 0.576)       // #8E8E93
    static let darkOutlineVariant = Color(red: 0.227, green: 0.227, blue: 0.235) // #3A3A3C
    static let darkPrimaryContainer = Color(red: 0.2, green: 0.2, blue: 0.2)     // #333333
    static let darkSecondaryContainer = Color(red: 0.2, green: 0.2, blue: 0.2)   // #333333

    // MARK: - Light Mode Colors (Matching Android)
    static let lightBackground = Color(red: 0.96, green: 0.96, blue: 0.969)      // #F5F5F7
    static let lightSurface = Color.white                                          // #FFFFFF
    static let lightSurfaceVariant = Color(red: 0.91, green: 0.91, blue: 0.929)  // #E8E8ED
    static let lightPrimary = Color.black                                          // #000000
    static let lightOnPrimary = Color.white                                        // #FFFFFF
    static let lightOnSurface = Color(red: 0.114, green: 0.114, blue: 0.122)     // #1D1D1F
    static let lightOnSurfaceVariant = Color(red: 0.239, green: 0.239, blue: 0.259) // #3D3D42
    static let lightOutline = Color(red: 0.451, green: 0.451, blue: 0.471)       // #737378
    static let lightOutlineVariant = Color(red: 0.784, green: 0.784, blue: 0.8)  // #C8C8CC
    static let lightPrimaryContainer = Color(red: 0.91, green: 0.91, blue: 0.929) // #E8E8ED
    static let lightSecondaryContainer = Color(red: 0.851, green: 0.851, blue: 0.867) // #D9D9DE

    // MARK: - Shared Accent Colors
    static let success = Color(red: 0.188, green: 0.82, blue: 0.345)             // #30D158
    static let error = Color(red: 1.0, green: 0.271, blue: 0.227)                // #FF453A
    static let accentLight = Color.black                                           // #000000
    static let accentDark = Color.white                                            // #FFFFFF

    // MARK: - Glass Material Colors
    static let glassDarkBackground = Color.white.opacity(0.05)
    static let glassDarkBorder = Color.white.opacity(0.08)
    static let glassLightBackground = Color.black.opacity(0.03)
    static let glassLightBorder = Color.black.opacity(0.06)

    // MARK: - Theme-Aware Colors
    static func background(for colorScheme: ColorScheme) -> Color {
        colorScheme == .dark ? darkBackground : lightBackground
    }

    static func surface(for colorScheme: ColorScheme) -> Color {
        colorScheme == .dark ? darkSurface : lightSurface
    }

    static func surfaceVariant(for colorScheme: ColorScheme) -> Color {
        colorScheme == .dark ? darkSurfaceVariant : lightSurfaceVariant
    }

    static func primary(for colorScheme: ColorScheme) -> Color {
        colorScheme == .dark ? darkPrimary : lightPrimary
    }

    static func onPrimary(for colorScheme: ColorScheme) -> Color {
        colorScheme == .dark ? darkOnPrimary : lightOnPrimary
    }

    static func onSurface(for colorScheme: ColorScheme) -> Color {
        colorScheme == .dark ? darkOnSurface : lightOnSurface
    }

    static func onSurfaceVariant(for colorScheme: ColorScheme) -> Color {
        colorScheme == .dark ? darkOnSurfaceVariant : lightOnSurfaceVariant
    }

    static func outline(for colorScheme: ColorScheme) -> Color {
        colorScheme == .dark ? darkOutline : lightOutline
    }

    static func outlineVariant(for colorScheme: ColorScheme) -> Color {
        colorScheme == .dark ? darkOutlineVariant : lightOutlineVariant
    }

    static func primaryContainer(for colorScheme: ColorScheme) -> Color {
        colorScheme == .dark ? darkPrimaryContainer : lightPrimaryContainer
    }

    static func secondaryContainer(for colorScheme: ColorScheme) -> Color {
        colorScheme == .dark ? darkSecondaryContainer : lightSecondaryContainer
    }

    static func accent(for colorScheme: ColorScheme) -> Color {
        colorScheme == .dark ? accentDark : accentLight
    }

    static func glassBackground(for colorScheme: ColorScheme) -> Color {
        colorScheme == .dark ? glassDarkBackground : glassLightBackground
    }

    static func glassBorder(for colorScheme: ColorScheme) -> Color {
        colorScheme == .dark ? glassDarkBorder : glassLightBorder
    }
}

// MARK: - Glass Card Modifier
struct GlassCardModifier: ViewModifier {
    @Environment(\.colorScheme) private var colorScheme
    var cornerRadius: CGFloat = 16
    var padding: CGFloat = 16

    func body(content: Content) -> some View {
        content
            .padding(padding)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(
                ZStack {
                    // Base surface
                    GlassTheme.surfaceVariant(for: colorScheme)
                    // Glass shimmer effect
                    LinearGradient(
                        colors: [
                            Color.white.opacity(colorScheme == .dark ? 0.03 : 0.05),
                            Color.clear
                        ],
                        startPoint: .topLeading,
                        endPoint: .bottomTrailing
                    )
                }
            )
            .clipShape(RoundedRectangle(cornerRadius: cornerRadius, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
                    .stroke(
                        GlassTheme.outlineVariant(for: colorScheme).opacity(0.5),
                        lineWidth: 0.5
                    )
            )
    }
}

// MARK: - Glass Background Modifier
struct GlassBackgroundModifier: ViewModifier {
    @Environment(\.colorScheme) private var colorScheme

    func body(content: Content) -> some View {
        content
            .background(
                ZStack {
                    // Solid background
                    GlassTheme.background(for: colorScheme).ignoresSafeArea()
                    // Subtle glass overlay
                    LinearGradient(
                        colors: [
                            Color.white.opacity(colorScheme == .dark ? 0.02 : 0.04),
                            Color.clear,
                            Color.black.opacity(colorScheme == .dark ? 0.01 : 0.02)
                        ],
                        startPoint: .top,
                        endPoint: .bottom
                    )
                    .ignoresSafeArea()
                }
            )
    }
}

// MARK: - Glass Navigation Bar Modifier
struct GlassNavigationBarModifier: ViewModifier {
    @Environment(\.colorScheme) private var colorScheme

    func body(content: Content) -> some View {
        content
            .toolbarBackground(
                .ultraThinMaterial,
                for: .navigationBar
            )
            .toolbarBackground(
                .visible,
                for: .navigationBar
            )
            .toolbarColorScheme(
                colorScheme == .dark ? .dark : .light,
                for: .navigationBar
            )
    }
}

// MARK: - Glass Tab Bar Modifier
struct GlassTabBarModifier: ViewModifier {
    @Environment(\.colorScheme) private var colorScheme

    func body(content: Content) -> some View {
        content
            .toolbarBackground(
                .ultraThinMaterial,
                for: .tabBar
            )
            .toolbarBackground(
                .visible,
                for: .tabBar
            )
            .toolbarColorScheme(
                colorScheme == .dark ? .dark : .light,
                for: .tabBar
            )
    }
}

// MARK: - View Extensions
extension View {
    func glassCard(cornerRadius: CGFloat = 16, padding: CGFloat = 16) -> some View {
        modifier(GlassCardModifier(cornerRadius: cornerRadius, padding: padding))
    }

    func glassBackground() -> some View {
        modifier(GlassBackgroundModifier())
    }

    func glassNavigationBar() -> some View {
        modifier(GlassNavigationBarModifier())
    }

    func glassTabBar() -> some View {
        modifier(GlassTabBarModifier())
    }
}

// MARK: - Device Icon Helper (Matching Android)
func deviceIconName(for type: String) -> String {
    let normalized = type.lowercased()
    if normalized.contains("android") || normalized.contains("phone") || normalized.contains("mobile") {
        return "iphone.gen3.radiowaves.left.and.right"
    }
    if normalized.contains("ios") || normalized.contains("iphone") {
        return "iphone"
    }
    if normalized.contains("ipad") || normalized.contains("tablet") {
        return "ipad"
    }
    if normalized.contains("mac") || normalized.contains("macos") {
        return "laptopcomputer"
    }
    if normalized.contains("windows") {
        return "desktopcomputer"
    }
    if normalized.contains("linux") {
        return "terminal"
    }
    if normalized.contains("tv") {
        return "tv"
    }
    if normalized.contains("watch") {
        return "applewatch"
    }
    return "display"
}

// MARK: - Monochrome Toggle Style
struct MonochromeToggleStyle: ToggleStyle {
    @Environment(\.colorScheme) private var colorScheme

    func makeBody(configuration: Configuration) -> some View {
        Button {
            withAnimation(.easeInOut(duration: 0.2)) {
                configuration.isOn.toggle()
            }
        } label: {
            HStack {
                configuration.label
                Spacer()
                ZStack {
                    Capsule()
                        .fill(configuration.isOn
                            ? GlassTheme.primary(for: colorScheme)
                            : GlassTheme.outlineVariant(for: colorScheme))
                        .frame(width: 51, height: 31)

                    Circle()
                        .fill(configuration.isOn
                            ? GlassTheme.onPrimary(for: colorScheme)
                            : .white)
                        .frame(width: 27, height: 27)
                        .offset(x: configuration.isOn ? 12 : -12)
                        .shadow(color: .black.opacity(0.15), radius: 1, x: 0, y: 1)
                }
            }
        }
        .buttonStyle(.plain)
    }
}
