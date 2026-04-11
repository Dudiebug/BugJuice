// Custom build: feeds our own app.manifest to tauri-build so the
// resulting bugjuice.exe gets the correct manifest embedded.
//
// The manifest uses asInvoker — no UAC prompt. Privileged EMI reads
// are handled by the bugjuice-svc Windows service (runs as SYSTEM).
// The app connects to the service over a named pipe.
//
// We do NOT use the `embed-manifest` crate here -- tauri-build already
// generates a resource.lib with a default manifest, so adding a second
// one causes CVT1100 "duplicate resource" at link time. Tauri exposes
// `WindowsAttributes::app_manifest(xml)` for exactly this case.
//
// IMPORTANT: app.manifest MUST be pure ASCII. tauri-winres writes each
// line into a .rc file as a quoted string, and the Windows resource
// compiler treats that file as Windows-1252 / the system codepage, not
// UTF-8. A single non-ASCII byte (e.g. an em dash in a comment) causes
// the RC compiler to silently drop the surrounding XML element(s) -- in
// our case it wiped out the Common-Controls v6 <dependency> block and
// the exe then crashed at startup with "Entry Point Not Found:
// TaskDialogIndirect". Keep the manifest ASCII-only.

fn main() {
    // Without this, cargo won't re-run build.rs when app.manifest is
    // edited, and the stale resource.lib gets reused. That exact trap
    // cost us an afternoon of "the fix isn't landing" debugging.
    println!("cargo:rerun-if-changed=app.manifest");

    let mut attrs = tauri_build::Attributes::new();

    #[cfg(windows)]
    {
        let manifest = std::fs::read_to_string("app.manifest")
            .expect("failed to read app.manifest next to build.rs");
        attrs = attrs.windows_attributes(
            tauri_build::WindowsAttributes::new().app_manifest(manifest),
        );
    }

    tauri_build::try_build(attrs).expect("failed to run tauri-build");
}
