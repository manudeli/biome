use biome_analyze::{AnalyzerAction, AnalyzerConfiguration, AnalyzerOptions, AnalyzerRules};
use biome_console::fmt::{Formatter, Termcolor};
use biome_console::markup;
use biome_diagnostics::termcolor::Buffer;
use biome_diagnostics::{DiagnosticExt, Error, PrintDiagnostic};
use biome_json_parser::{JsonParserOptions, ParseDiagnostic};
use biome_project::PackageJson;
use biome_rowan::{SyntaxKind, SyntaxNode, SyntaxSlot};
use biome_service::configuration::to_analyzer_rules;
use biome_service::settings::{Language, WorkspaceSettings};
use biome_service::PartialConfiguration;
use json_comments::StripComments;
use similar::TextDiff;
use std::ffi::{c_int, OsStr};
use std::fmt::Write;
use std::path::Path;
use std::sync::Once;

pub fn scripts_from_json(extension: &OsStr, input_code: &str) -> Option<Vec<String>> {
    if extension == "json" || extension == "jsonc" {
        let input_code = StripComments::new(input_code.as_bytes());
        let scripts: Vec<String> = serde_json::from_reader(input_code).ok()?;
        Some(scripts)
    } else {
        None
    }
}

pub fn create_analyzer_options(
    input_file: &Path,
    diagnostics: &mut Vec<String>,
) -> AnalyzerOptions {
    let options = AnalyzerOptions {
        configuration: Default::default(),
        file_path: input_file.to_path_buf(),
    };
    // We allow a test file to configure its rule using a special
    // file with the same name as the test but with extension ".options.json"
    // that configures that specific rule.
    let mut analyzer_configuration = AnalyzerConfiguration {
        rules: AnalyzerRules::default(),
        globals: vec![],
    };
    let options_file = input_file.with_extension("options.json");
    if let Ok(json) = std::fs::read_to_string(options_file.clone()) {
        let deserialized = biome_deserialize::json::deserialize_from_json_str::<PartialConfiguration>(
            json.as_str(),
            JsonParserOptions::default(),
            "",
        );
        if deserialized.has_errors() {
            diagnostics.extend(
                deserialized
                    .into_diagnostics()
                    .into_iter()
                    .map(|diagnostic| {
                        diagnostic_to_string(
                            options_file.file_stem().unwrap().to_str().unwrap(),
                            &json,
                            diagnostic,
                        )
                    })
                    .collect::<Vec<_>>(),
            );
        } else {
            let configuration = deserialized.into_deserialized().unwrap_or_default();
            let mut settings = WorkspaceSettings::default();
            settings
                .merge_with_configuration(configuration, None, None, &[])
                .unwrap();
            analyzer_configuration.rules = to_analyzer_rules(&settings, input_file);
        }
    }

    AnalyzerOptions {
        configuration: analyzer_configuration,
        ..options
    }
}

pub fn load_manifest(input_file: &Path, diagnostics: &mut Vec<String>) -> Option<PackageJson> {
    let options_file = input_file.with_extension("package.json");
    if let Ok(json) = std::fs::read_to_string(options_file.clone()) {
        let deserialized = biome_deserialize::json::deserialize_from_json_str::<PackageJson>(
            json.as_str(),
            JsonParserOptions::default(),
            "",
        );
        if deserialized.has_errors() {
            diagnostics.extend(
                deserialized
                    .into_diagnostics()
                    .into_iter()
                    .map(|diagnostic| {
                        diagnostic_to_string(
                            options_file.file_stem().unwrap().to_str().unwrap(),
                            &json,
                            diagnostic,
                        )
                    })
                    .collect::<Vec<_>>(),
            );
        } else {
            return deserialized.into_deserialized();
        }
    }
    None
}

pub fn diagnostic_to_string(name: &str, source: &str, diag: Error) -> String {
    let error = diag.with_file_path(name).with_file_source_code(source);
    let text = markup_to_string(biome_console::markup! {
        {PrintDiagnostic::verbose(&error)}
    });

    text
}

fn markup_to_string(markup: biome_console::Markup) -> String {
    let mut buffer = Vec::new();
    let mut write =
        biome_console::fmt::Termcolor(biome_diagnostics::termcolor::NoColor::new(&mut buffer));
    let mut fmt = Formatter::new(&mut write);
    fmt.write_markup(markup).unwrap();

    String::from_utf8(buffer).unwrap()
}

// Check that all red / green nodes have correctly been released on exit
extern "C" fn check_leaks() {
    if let Some(report) = biome_rowan::check_live() {
        panic!("\n{report}")
    }
}
pub fn register_leak_checker() {
    // Import the atexit function from libc
    extern "C" {
        fn atexit(f: extern "C" fn()) -> c_int;
    }

    // Use an atomic Once to register the check_leaks function to be called
    // when the process exits
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        countme::enable(true);
        atexit(check_leaks);
    });
}

pub fn code_fix_to_string<L: Language>(source: &str, action: AnalyzerAction<L>) -> String {
    let (_, text_edit) = action.mutation.as_text_edits().unwrap_or_default();

    let output = text_edit.new_string(source);

    let diff = TextDiff::from_lines(source, &output);

    let mut diff = diff.unified_diff();
    diff.context_radius(3);

    diff.to_string()
}

/// The test runner for the analyzer is currently designed to have a
/// one-to-one mapping between test case and analyzer rules.
/// So each testing file will be run through the analyzer with only the rule
/// corresponding to the directory name. E.g., `style/useWhile/test.js`
/// will be analyzed with just the `style/useWhile` rule.
pub fn parse_test_path(file: &Path) -> (&str, &str) {
    let rule_folder = file.parent().unwrap();
    let rule_name = rule_folder.file_name().unwrap();

    let group_folder = rule_folder.parent().unwrap();
    let group_name = group_folder.file_name().unwrap();

    (group_name.to_str().unwrap(), rule_name.to_str().unwrap())
}

/// This check is used in the parser test to ensure it doesn't emit
/// bogus nodes without diagnostics, and in the analyzer tests to
/// check the syntax trees resulting from code actions are correct
pub fn has_bogus_nodes_or_empty_slots<L: biome_rowan::Language>(node: &SyntaxNode<L>) -> bool {
    node.descendants().any(|descendant| {
        let kind = descendant.kind();
        if kind.is_bogus() {
            return true;
        }

        if kind.is_list() {
            return descendant
                .slots()
                .any(|slot| matches!(slot, SyntaxSlot::Empty));
        }

        false
    })
}

/// This function analyzes the parsing result of a file and panic with a
/// detailed message if it contains any error-level diagnostic, bogus nodes,
/// empty list slots or missing required children
pub fn assert_errors_are_absent<L: Language>(
    program: &SyntaxNode<L>,
    diagnostics: &[ParseDiagnostic],
    path: &Path,
) {
    let debug_tree = format!("{:?}", program);
    let has_missing_children = debug_tree.contains("missing (required)");

    if diagnostics.is_empty() && !has_bogus_nodes_or_empty_slots(program) && !has_missing_children {
        return;
    }

    let mut buffer = Buffer::no_color();
    for diagnostic in diagnostics {
        let error = diagnostic
            .clone()
            .with_file_path(path.to_str().unwrap())
            .with_file_source_code(program.to_string());
        Formatter::new(&mut Termcolor(&mut buffer))
            .write_markup(markup! {
                {PrintDiagnostic::verbose(&error)}
            })
            .unwrap();
    }

    panic!("There should be no errors in the file {:?} but the following errors where present:\n{}\n\nParsed tree:\n{:#?}",
           path.display(),
           std::str::from_utf8(buffer.as_slice()).unwrap(),
           &program
    );
}

pub fn write_analyzer_snapshot(
    snapshot: &mut String,
    input_code: &str,
    diagnostics: &[String],
    code_fixes: &[String],
    markdown_language: &str,
) {
    writeln!(snapshot, "# Input").unwrap();
    writeln!(snapshot, "```{markdown_language}").unwrap();
    writeln!(snapshot, "{}", input_code).unwrap();
    writeln!(snapshot, "```").unwrap();
    writeln!(snapshot).unwrap();

    if !diagnostics.is_empty() {
        writeln!(snapshot, "# Diagnostics").unwrap();
        for diagnostic in diagnostics {
            writeln!(snapshot, "```").unwrap();
            writeln!(snapshot, "{}", diagnostic).unwrap();
            writeln!(snapshot, "```").unwrap();
            writeln!(snapshot).unwrap();
        }
    }

    if !code_fixes.is_empty() {
        writeln!(snapshot, "# Actions").unwrap();
        for action in code_fixes {
            writeln!(snapshot, "```diff").unwrap();
            writeln!(snapshot, "{}", action).unwrap();
            writeln!(snapshot, "```").unwrap();
            writeln!(snapshot).unwrap();
        }
    }
}

pub fn write_transformation_snapshot(
    snapshot: &mut String,
    input_code: &str,
    transformations: &[String],
    extension: &str,
) {
    writeln!(snapshot, "# Input").unwrap();
    writeln!(snapshot, "```{}", extension).unwrap();
    writeln!(snapshot, "{}", input_code).unwrap();
    writeln!(snapshot, "```").unwrap();
    writeln!(snapshot).unwrap();

    if !transformations.is_empty() {
        writeln!(snapshot, "# Transformations").unwrap();
        for transformation in transformations {
            writeln!(snapshot, "```{}", extension).unwrap();
            writeln!(snapshot, "{}", transformation).unwrap();
            writeln!(snapshot, "```").unwrap();
            writeln!(snapshot).unwrap();
        }
    }
}

pub enum CheckActionType {
    Suppression,
    Lint,
}

impl CheckActionType {
    pub const fn is_suppression(&self) -> bool {
        matches!(self, Self::Suppression)
    }
}
