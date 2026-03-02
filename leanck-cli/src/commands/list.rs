use crate::args::LiveArgs;

use clap::builder::TypedValueParser;
use leanck_core::api::{KindSelector, ListParams};

use crate::commands::output::print_json;
use leanck_client::render::render_list;

fn kind_parser() -> impl TypedValueParser<Value = KindSelector> {
    clap::builder::PossibleValuesParser::new(KindSelector::ALL.iter().map(|kind| kind.as_str()))
        .try_map(|value| value.parse::<KindSelector>())
}

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// File to list (workspace-relative path or `file://` URI).
    pub(crate) file: String,

    /// Restrict to these Lean keywords (`theorem`, `def`, `lemma`, …) or LSP
    /// kinds (`namespace`, `class`, …). Repeatable or comma-separated. Empty lists everything.
    #[arg(long = "kind", value_parser = kind_parser(), value_delimiter = ',')]
    pub(crate) kind: Vec<KindSelector>,

    #[command(flatten)]
    pub(crate) live: LiveArgs,
}

pub(crate) async fn run(args: Args) -> anyhow::Result<()> {
    let client = args.live.client()?;
    let params = ListParams {
        file: args.file.clone(),
        kind: (!args.kind.is_empty()).then(|| args.kind.clone()),
    };
    let body = client.list(&params, args.live.request_options()).await?;
    if args.live.output.json {
        print_json(&body)?;
    } else {
        println!("{}", render_list(&body));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, Cmd};
    use clap::{CommandFactory, Parser};

    #[test]
    fn list_kinds_are_closed_canonical_and_repeatable() {
        let Cmd::List(args) = Cli::try_parse_from([
            "leanck",
            "list",
            "Basic.lean",
            "--kind",
            "theorem,enum_member",
            "--kind",
            "type_parameter",
        ])
        .unwrap()
        .command
        else {
            panic!("expected list")
        };
        assert_eq!(
            args.kind,
            [
                KindSelector::Theorem,
                KindSelector::EnumMember,
                KindSelector::TypeParameter
            ]
        );
        for kind in KindSelector::ALL {
            assert!(
                Cli::try_parse_from(["leanck", "list", "Basic.lean", "--kind", kind.as_str()])
                    .is_ok()
            );
        }
        for typo in ["enumMember", "typeParameter", "thorem", "THEOREM"] {
            assert!(Cli::try_parse_from(["leanck", "list", "Basic.lean", "--kind", typo]).is_err());
        }
        let mut cmd = Cli::command();
        let help = cmd
            .find_subcommand_mut("list")
            .unwrap()
            .render_long_help()
            .to_string();
        assert!(help.contains("enum_member"));
        assert!(help.contains("type_parameter"));
    }
}
