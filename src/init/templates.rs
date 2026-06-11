//! Compile-time-embedded `dairo init` template files.
//!
//! Templates are embedded with `include_str!` so `dairo init` works offline,
//! air-gapped, and deterministically: there is no CDN fetch in the path and the
//! SDK versions pinned in generated manifests always match what this CLI release
//! was tested against (templates ship *with* the binary). Each entry maps a
//! framework and its output-relative path to the literal template bytes; the
//! `.tmpl` suffix on disk is stripped from the output path here.
//!
//! Only files written verbatim (with `{{var}}` substitution) live here. Files
//! that are *merged* or *appended* into the user's project — `package.json`,
//! `requirements.txt`, `.env.example`, `.gitignore` — are generated
//! programmatically in `manifest.rs` so existing user content is preserved.

use crate::cli::Framework;

/// One embedded template: the output path (relative to the target dir) and the
/// literal template contents.
pub struct EmbeddedTemplate {
    /// Output path relative to the target project directory, with `.tmpl`
    /// already stripped.
    pub out_path: &'static str,
    /// Raw template contents with `{{var}}` placeholders.
    pub contents: &'static str,
}

/// Returns the embedded code/doc templates for `framework`. The list excludes
/// the merge/append files (`package.json`, `requirements.txt`, `.env.example`,
/// `.gitignore`), which `manifest.rs` builds from existing project content.
pub fn templates_for(framework: Framework) -> &'static [EmbeddedTemplate] {
    match framework {
        Framework::Next => NEXT,
        Framework::Express => EXPRESS,
        Framework::Hono => HONO,
        Framework::CloudflareWorkers => CLOUDFLARE_WORKERS,
        Framework::Fastapi => FASTAPI,
        Framework::Flask => FLASK,
        Framework::GoHttp => GO_HTTP,
    }
}

const NEXT: &[EmbeddedTemplate] = &[
    EmbeddedTemplate {
        out_path: "lib/dairo.ts",
        contents: include_str!("templates/next/lib/dairo.ts.tmpl"),
    },
    EmbeddedTemplate {
        out_path: "app/api/dairo/send/route.ts",
        contents: include_str!("templates/next/app/api/dairo/send/route.ts.tmpl"),
    },
    EmbeddedTemplate {
        out_path: "app/api/dairo/webhook/route.ts",
        contents: include_str!("templates/next/app/api/dairo/webhook/route.ts.tmpl"),
    },
    EmbeddedTemplate {
        out_path: "DAIRO.md",
        contents: include_str!("templates/next/DAIRO.md.tmpl"),
    },
];

const EXPRESS: &[EmbeddedTemplate] = &[
    EmbeddedTemplate {
        out_path: "src/dairo.ts",
        contents: include_str!("templates/express/src/dairo.ts.tmpl"),
    },
    EmbeddedTemplate {
        out_path: "src/server.ts",
        contents: include_str!("templates/express/src/server.ts.tmpl"),
    },
    EmbeddedTemplate {
        out_path: "DAIRO.md",
        contents: include_str!("templates/express/DAIRO.md.tmpl"),
    },
];

const HONO: &[EmbeddedTemplate] = &[
    EmbeddedTemplate {
        out_path: "src/index.ts",
        contents: include_str!("templates/hono/src/index.ts.tmpl"),
    },
    EmbeddedTemplate {
        out_path: "DAIRO.md",
        contents: include_str!("templates/hono/DAIRO.md.tmpl"),
    },
];

const CLOUDFLARE_WORKERS: &[EmbeddedTemplate] = &[
    EmbeddedTemplate {
        out_path: "src/index.ts",
        contents: include_str!("templates/cloudflare-workers/src/index.ts.tmpl"),
    },
    EmbeddedTemplate {
        out_path: "wrangler.toml",
        contents: include_str!("templates/cloudflare-workers/wrangler.toml.tmpl"),
    },
    EmbeddedTemplate {
        out_path: "DAIRO.md",
        contents: include_str!("templates/cloudflare-workers/DAIRO.md.tmpl"),
    },
];

const FASTAPI: &[EmbeddedTemplate] = &[
    EmbeddedTemplate {
        out_path: "main.py",
        contents: include_str!("templates/fastapi/main.py.tmpl"),
    },
    EmbeddedTemplate {
        out_path: "DAIRO.md",
        contents: include_str!("templates/fastapi/DAIRO.md.tmpl"),
    },
];

const FLASK: &[EmbeddedTemplate] = &[
    EmbeddedTemplate {
        out_path: "app.py",
        contents: include_str!("templates/flask/app.py.tmpl"),
    },
    EmbeddedTemplate {
        out_path: "DAIRO.md",
        contents: include_str!("templates/flask/DAIRO.md.tmpl"),
    },
];

const GO_HTTP: &[EmbeddedTemplate] = &[
    EmbeddedTemplate {
        out_path: "main.go",
        contents: include_str!("templates/go-http/main.go.tmpl"),
    },
    EmbeddedTemplate {
        out_path: "go.mod",
        contents: include_str!("templates/go-http/go.mod.tmpl"),
    },
    EmbeddedTemplate {
        out_path: "DAIRO.md",
        contents: include_str!("templates/go-http/DAIRO.md.tmpl"),
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_framework_has_a_webhook_handler_and_readme() {
        for framework in [
            Framework::Next,
            Framework::Express,
            Framework::Hono,
            Framework::CloudflareWorkers,
            Framework::Fastapi,
            Framework::Flask,
            Framework::GoHttp,
        ] {
            let templates = templates_for(framework);
            assert!(
                templates.iter().any(|t| t.out_path == "DAIRO.md"),
                "{framework} is missing DAIRO.md"
            );
            // The webhook stub must verify against the raw body in every
            // framework: the verify helper name appears in exactly one file.
            let has_verify = templates.iter().any(|t| {
                t.contents.contains("verifyWebhookRequest")
                    || t.contents.contains("verifyWebhook")
                    || t.contents.contains("verify_webhook")
                    || t.contents.contains("VerifyWebhookRequest")
            });
            assert!(has_verify, "{framework} has no webhook verify call");
        }
    }

    #[test]
    fn out_paths_have_no_tmpl_suffix() {
        for framework in [Framework::Next, Framework::GoHttp] {
            for template in templates_for(framework) {
                assert!(
                    !template.out_path.ends_with(".tmpl"),
                    "{} should not keep the .tmpl suffix",
                    template.out_path
                );
            }
        }
    }
}
