# Version is injected by CI: rpmbuild --define "pkgver X.Y.Z"
%{!?pkgver: %global pkgver 1.1.0}
%global debug_package %{nil}

Name:           pulsedeck
Version:        %{pkgver}
Release:        1%{?dist}
Summary:        Native hardware telemetry and system observatory (Slint GUI + TUI)
License:        LicenseRef-Unknown
URL:            https://github.com/m4ce-w1ndu/pulsedeck
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  gcc
BuildRequires:  pkgconfig(fontconfig)
BuildRequires:  pkgconfig(freetype2)

# Loaded at runtime via dlopen, so rpm cannot detect them automatically.
Requires:       libxkbcommon.so.0()(64bit)
Requires:       libxkbcommon-x11.so.0()(64bit)
Recommends:     libwayland-client.so.0()(64bit)
Recommends:     libEGL.so.1()(64bit)
Recommends:     libX11.so.6()(64bit)
Recommends:     libXcursor.so.1()(64bit)

%description
PulseDeck is a fast, native hardware telemetry and system observatory built in
Rust with a Slint desktop GUI and an interactive terminal UI (--tui).

%prep
%autosetup -n %{name}-%{version}

%build
cargo build --release --locked

# No %%check: the unit test inspects live host state (sockets, sensors) that build
# sandboxes lack. CI runs it on a real runner and smoke-tests the installed package.

%install
install -Dm755 target/release/%{name} %{buildroot}%{_bindir}/%{name}
install -Dm644 packaging/linux/io.pulsedeck.PulseDeck.desktop \
  %{buildroot}%{_datadir}/applications/io.pulsedeck.PulseDeck.desktop
install -Dm644 packaging/linux/pulsedeck.svg \
  %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/pulsedeck.svg

%files
%doc README.md
%{_bindir}/%{name}
%{_datadir}/applications/io.pulsedeck.PulseDeck.desktop
%{_datadir}/icons/hicolor/scalable/apps/pulsedeck.svg
