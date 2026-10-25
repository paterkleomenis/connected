Name:           connected-desktop
Version:        3.5.2
Release:        1%{?dist}
Summary:        High-speed, offline, cross-platform ecosystem bridging devices

License:        MIT OR Apache-2.0
URL:            https://github.com/paterkleomenis/connected
Source0:        %{url}/releases/download/%{version}/connected-desktop-linux-%{_arch}
Source1:        connected-desktop.desktop
Source2:        io.github.paterkleomenis.Connected.png
Source3:        LICENSE-MIT
Source4:        LICENSE-APACHE

BuildRequires:  gcc

Requires:       gtk3
Requires:       webkit2gtk4.1
Requires:       libappindicator-gtk3
Requires:       openssl
Requires:       dbus

Provides:       connected-desktop = %{version}-%{release}
Obsoletes:      connected-desktop < %{version}-%{release}

%description
Connected is a cross-platform application that allows you to seamlessly share
files, sync clipboards, and control your devices over a local network.

Features:
- High-speed file transfer via QUIC protocol
- Clipboard synchronization
- Remote input and media control
- End-to-end encrypted communication
- Automatic device discovery via mDNS

%prep
# Binary package - no source to prep

%build
# Binary package - no build needed

%install
# Install binary
install -Dm755 %{SOURCE0} %{buildroot}%{_bindir}/%{name}

# Install desktop file
install -Dm644 %{SOURCE1} %{buildroot}%{_datadir}/applications/%{name}.desktop

# Install icon
install -Dm644 %{SOURCE2} %{buildroot}%{_datadir}/icons/hicolor/512x512/apps/%{name}.png

# Install licenses
install -Dm644 %{SOURCE3} %{buildroot}%{_datadir}/licenses/%{name}/LICENSE-MIT
install -Dm644 %{SOURCE4} %{buildroot}%{_datadir}/licenses/%{name}/LICENSE-APACHE

%files
%license %{_datadir}/licenses/%{name}/LICENSE-MIT
%license %{_datadir}/licenses/%{name}/LICENSE-APACHE
%{_bindir}/%{name}
%{_datadir}/applications/%{name}.desktop
%{_datadir}/icons/hicolor/512x512/apps/%{name}.png

%changelog
* Sat Oct 24 2026 Connected Team <paterkleomenis@protonmail.com> - 3.5.2-1
- Update to 3.5.2

* Fri Oct 23 2026 Connected Team <paterkleomenis@protonmail.com> - 3.5.1-1
- Update to 3.5.1

* Wed Oct 21 2026 Connected Team <paterkleomenis@protonmail.com> - 3.5.0-1
- Update to 3.5.0

* Mon Oct 19 2026 Connected Team <paterkleomenis@protonmail.com> - 3.4.4-1
- Update to 3.4.4
- Include trust-chain hardening, bounded stream timeouts, IPv4 discovery
  preference, and Android foreground-service timeout fixes

* Fri Jun 26 2026 Connected Team <paterkleomenis@protonmail.com> - 3.3.3-1
- Initial RPM package
- Binary package for Fedora
