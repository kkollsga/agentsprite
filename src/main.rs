//! pxl: CLI entry. Parses arguments, dispatches to command modules, prints text or one JSON object.

use agentsprite::cli::{Cli, Command, SkillCmd, SnippetCmd, TemplateCmd, TilesCmd};
use agentsprite::output::{Out, Result};
use agentsprite::*;
use clap::Parser;

fn dispatch(cli: &Cli, out: &mut Out) -> Result<()> {
    match &cli.command {
        Command::Load(a) => load::run_load(a, out),
        Command::Reload(a) => load::run_reload(a, out),
        Command::Template(TemplateCmd::Suggest(a)) => suggest::run_suggest(a, out),
        Command::Template(TemplateCmd::Remap(a)) => edit::run_remap(a, out),
        Command::Template(TemplateCmd::Set(a)) => template::run_set(a, out),
        Command::Template(TemplateCmd::AddSet(a)) => template::run_add_set(a, out),
        Command::Set(a) => edit::run_set(a, out),
        Command::Row(a) => edit::run_row(a, out),
        Command::Snippet(SnippetCmd::Save(a)) => snippet::run_save(a, out),
        Command::Snippet(SnippetCmd::List(a)) => snippet::run_list(a, out),
        Command::Stamp(a) => edit::run_stamp(a, out),
        Command::Lineup(a) => lineup::run(a, out),
        Command::Pattern(a) => edit::run_pattern(a, out),
        Command::Move(a) => edit::run_move(a, out),
        Command::Nudge(a) => edit::run_nudge(a, out),
        Command::Outline(a) => edit::run_outline(a, out),
        Command::Fix(a) => edit::run_fix(a, out),
        Command::View(a) => show::run_view(a, out),
        Command::Patch(a) => edit::run_patch(a, out),
        Command::Line(a) => edit::run_line(a, out),
        Command::Rect(a) => edit::run_rect(a, out),
        Command::Fill(a) => edit::run_fill(a, out),
        Command::Replace(a) => edit::run_replace(a, out),
        Command::Copy(a) => edit::run_copy(a, out),
        Command::Mirror(a) => edit::run_mirror(a, out),
        Command::Flip(a) => edit::run_flip(a, out),
        Command::WriteGrid(a) => edit::run_write_grid(a, out),
        Command::Candidates(a) => candidates::run_candidates(a, out),
        Command::Pick(a) => edit::run_pick(a, out),
        Command::Finish(a) => edit::run_finish(a, out),
        Command::Lock(a) => edit::run_lock(a, true, out),
        Command::Unlock(a) => edit::run_lock(a, false, out),
        Command::Apply(a) => edit::run_apply(a, out),
        Command::Undo(a) => journal::run_undo(a, out),
        Command::Clean(a) => edit::run_clean(a, out),
        Command::Show(a) => show::run_show(a, out),
        Command::Compare(a) => compare::run_compare(a, out),
        Command::Diff(a) => compare::run_diff(a, out),
        Command::Cells(a) => {
            if a.part == "dedupe" {
                let part = a.rest.first().cloned().ok_or_else(|| output::PxlError::usage("cells dedupe needs a part").with_hint("pxl cells dedupe PART --max N [--apply]"))?;
                let max = a.max.ok_or_else(|| output::PxlError::usage("cells dedupe needs --max N"))?;
                cellmem::run_dedupe(&cli::DedupeArgs { part, max, apply: a.apply, force_approved: a.force_approved }, out)
            } else {
                cells::run_cells(&cli::CellsShowArgs { part: a.part.clone(), frame: a.rest.first().cloned(), png: a.png.clone(), zoom: a.zoom, set: a.set.clone() }, out)
            }
        }
        Command::Palette(cli::PaletteCmd::Fit(a)) => fit::run_fit(a, out),
        Command::Budget(a) => cellmem::run_budget(a, out),
        Command::Plan(a) => plan::run_plan(a, out),
        Command::Meta(a) => dmanifest::run_meta(a, out),
        Command::Sheet(a) => deliver::run_sheet(a, out),
        Command::Package(a) => deliver::run_package(a, out),
        Command::VerifyPackage(a) => deliver::run_verify(a, out),
        Command::Deliver(a) => deliver::run_deliver(a, out),
        Command::Changes(a) => deliver::run_changes(a, out),
        Command::Qa(a) => sourceqa::run_qa(a, out),
        Command::Palette(cli::PaletteCmd::Plan(a)) => plan::run_palette_plan(a, out),
        Command::Scene(a) if matches!(a.name.as_str(), "set" | "add-part" | "remove-part") => manedit::run_scene_edit(a, out),
        Command::Scene(a) => scene::run_scene(a, out),
        Command::Palette(cli::PaletteCmd::Slot(c)) => manedit::run_slot(c, out),
        Command::Delivery(a) => delivery::run(a, out),
        Command::Lint(a) => lint::run_lint(a, out),
        Command::Export(a) => export::run_export(a, out),
        Command::Build(a) => export::run_build(a, out),
        Command::Check(a) => export::run_check(a, out),
        Command::ExportPalettes(a) => scene::run_export_palettes(a, out),
        Command::Import(a) => import::run_import(a, out),
        Command::Tiles(t) => match t {
            TilesCmd::Stats(a) => tiles::run_stats(a, out),
            TilesCmd::Fit(a) => tiles::run_fit(a, out),
            TilesCmd::Dedupe(a) => tiles::run_dedupe(a, out),
            TilesCmd::Priority(a) => tiles::priority::run(a, out),
        },
        Command::Next(a) => queue::run_next(a, out),
        Command::Claim(a) => queue::run_claim(a, out),
        Command::Renew(a) => queue::run_renew(a, out),
        Command::Release(a) => queue::run_release(a, out),
        Command::Status(a) => queue::run_status(a, out),
        Command::Review(a) => queue::run_review(a, out),
        Command::Approve(a) => queue::run_approve(a, out),
        Command::Rework(a) => queue::run_rework(a, out),
        Command::Skill(SkillCmd::List) => skill::run_list(out),
        Command::Skill(SkillCmd::Show(a)) => skill::run_show(a, out),
        Command::Skill(SkillCmd::Install(a)) => skill::run_install(a, out),
        Command::Skill(SkillCmd::Uninstall(a)) => skill::run_uninstall(a, out),
        Command::Skill(SkillCmd::Path(a)) => skill::run_path(a, out),
        Command::Image(a) => project::run_image(a, out),
        Command::New(a) => project::run_new(a, out),
        Command::Ingest(a) => project::run_ingest(a, out),
        Command::Group(c) => project::run_group(c, out),
        Command::Asset(c) => manedit::run_asset(c, out),
        Command::Tree => project::run_tree(out),
        Command::Project(c) => project::run_project(c, out),
        Command::Versions(a) => versions::run_versions(a, out),
        Command::Revert(a) => versions::run_revert(a, out),
        Command::Task(c) => tasks::run_task(c, out),
        Command::Tasks(a) => tasks::run_tasks(a, out),
        Command::Board(a) => tasks::run_board(a, out),
        Command::Decide(a) => tasks::run_decide(a, out),
        Command::Exception(c) => tasks::run_exception(c, out),
        Command::Variant(c) => variants::run(c, out),
        Command::Brief(a) => tasks::run_brief(a, out),
        Command::Console(c) => console::run(c, out),
        Command::Migrate(a) => migrate::run(a, out),
        Command::Intake(a) => intake::run_intake(a, out),
        Command::Style(c) => style::run_style(c, out),
        Command::Milestone(c) => milestone::run_milestone(c, out),
        Command::Ask(a) => decisions::run_ask(a, out),
        Command::Decisions(a) => decisions::run_decisions(a, out),
        Command::Report(a) => report::run_report(a, out),
        Command::Guide { topic } => {
            let text = match topic.as_deref() {
                None => guide::TEXT,
                Some("project") => guide::PROJECT,
                Some(t) => return Err(output::PxlError::usage(format!("unknown guide page '{}'", t)).with_hint("pxl guide, or pxl guide project")),
            };
            out.emit(text, serde_json::json!({ "guide": text }));
            Ok(())
        }
    }
}

/// A command line that clap rejects. With `--json` (AC-I1) the error is one JSON object on stdout, exit 2.
fn usage_failure(e: clap::Error) -> ! {
    use clap::error::ErrorKind;
    let json = std::env::args().skip(1).any(|a| a == "--json");
    if json && !matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand) {
        let text = e.to_string();
        let body: Vec<&str> = text.lines().take_while(|l| !l.starts_with("Usage:") && !l.starts_with("For more information")).map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
        let msg = body.join(" ").trim_start_matches("error: ").to_string();
        let usage = text.lines().find(|l| l.starts_with("Usage:")).map(|l| l.to_string());
        let mut err = serde_json::json!({"kind": "usage", "message": msg});
        if let Some(u) = usage {
            err["hint"] = serde_json::json!(format!("{}; see `pxl guide` and `pxl <command> --help`", u));
        }
        println!("{}", serde_json::json!({"ok": false, "error": err}));
        std::process::exit(2);
    }
    e.exit()
}

fn main() {
    let cli = Cli::try_parse().unwrap_or_else(|e| usage_failure(e));
    if cli.no_version {
        versions::disable();
    }
    if cli.force_user_approved {
        status::allow_user_approved();
    }
    let mut out = Out::new(cli.json, cli.agent.clone());
    let res = dispatch(&cli, &mut out);
    let code = out.finish(res);
    std::process::exit(code);
}
