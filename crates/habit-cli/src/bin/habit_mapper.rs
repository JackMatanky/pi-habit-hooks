use std::{
    io::{self, Read},
    process,
};

use clap::Parser;
use habit_cli::{find_project_dir, load_enabled_plugins, load_project_config};
use habit_mapper::evaluate_and_render;

#[derive(Parser, Debug)]
#[command(name = "habit-mapper", about = "Habit Hooks Guide Mapper")]
struct Args {}

fn main() {
    let _args = Args::parse();
    let project_dir = find_project_dir();
    let project_config = load_project_config(&project_dir);
    let plugins = load_enabled_plugins(&project_dir, &project_config);

    let plugin_dirs: Vec<_> =
        plugins.iter().map(|p| p.plugin_dir.clone()).collect();

    let mut stdin_content = String::new();
    let _ = io::stdin().read_to_string(&mut stdin_content);

    let result = evaluate_and_render(
        &stdin_content,
        &project_dir,
        &plugin_dirs,
        &project_config,
    );

    print!("{}", result.output);
    process::exit(result.exit_code);
}
