use crate::cli::{
    Command, ElementCommand, KeyCommand, ListWindowsCommand, MouseClickCommand, OutputArgs,
    PlatformType, ScreenshotElementsCommand, ScreenshotSubcommand, TargetArgs, TimeoutArgs,
    TreeCommand, TreeFilterArgs, TypeCommand,
};
use crate::error::{CliError, CliResult};
use crate::operations::{self, OperationResult};
use crate::target;
use accessibility_core::accessibility::{ElementTree, TargetedAccessibility, TreeFilter};

pub async fn run_cli(cli: crate::Cli) -> CliResult<()> {
    match cli.command {
        Command::Tree(command) => run_tree(command).await,
        Command::Query(command) => {
            run_pollable(
                &command.target,
                &command.filter,
                command.timeout,
                PollOperation::Query {
                    selector: &command.selector,
                    output: &command.output,
                },
            )
            .await
        }
        Command::Click(command) => run_element(command, ElementAction::Click).await,
        Command::Press(command) => run_element(command, ElementAction::Press).await,
        Command::Focus(command) => run_element(command, ElementAction::Focus).await,
        Command::Blur(command) => run_element(command, ElementAction::Blur).await,
        Command::Type(command) => run_type(command).await,
        Command::Key(command) => run_key(command).await,
        Command::Hit(command) => {
            target::ensure_platform(
                &command.target,
                &[
                    PlatformType::MacOS,
                    PlatformType::Windows,
                    PlatformType::Linux,
                ],
                "hit",
            )?;
            let mut adapter = target::targeted_adapter(&command.target, false).await?;
            operations::tree::hit_test(&mut adapter, command.point.x, command.point.y).await
        }
        Command::MouseClick(command) => run_mouse_click(command).await,
        Command::Tap(command) => operations::device::tap(&command).await,
        Command::Swipe(command) => operations::device::swipe(&command).await,
        Command::LongPress(command) => operations::device::long_press(&command).await,
        Command::Button(command) => {
            operations::device::button(&command.target, command.button).await
        }
        Command::Launch(command) => operations::device::launch(&command).await,
        Command::Stop(command) => operations::device::stop(&command).await,
        Command::Notifications(command) => operations::device::notifications(&command.target).await,
        Command::QuickSettings(command) => {
            operations::device::quick_settings(&command.target).await
        }
        Command::Wake(command) => operations::device::wake(&command.target).await,
        Command::Sleep(command) => operations::device::sleep(&command.target).await,
        Command::ListWindows(command) => run_list_windows(command).await,
        Command::Listen(command) => {
            target::ensure_platform(
                &command.target,
                &[
                    PlatformType::MacOS,
                    PlatformType::Windows,
                    PlatformType::Linux,
                ],
                "listen",
            )?;
            let mut adapter = target::targeted_adapter(&command.target, true).await?;
            operations::events::listen(&mut adapter, command.filter.as_deref()).await
        }
        Command::Screenshot(command) => run_screenshot(command.command).await,
        Command::TestLoad(command) => operations::device::test_load(&command.target),
    }
}

async fn run_tree(command: TreeCommand) -> CliResult<()> {
    let mut adapter = target::targeted_adapter(&command.target, true).await?;
    let filter = command.filter.to_tree_filter();
    let tree = get_tree(&mut adapter, &filter).await?;
    operations::tree::print_tree(&adapter, &tree, &command.output);
    Ok(())
}

async fn run_element(command: ElementCommand, action: ElementAction) -> CliResult<()> {
    run_pollable(
        &command.target,
        &command.filter,
        command.timeout,
        match action {
            ElementAction::Click => PollOperation::Click {
                selector: &command.selector,
                action_name: "click",
            },
            ElementAction::Press => PollOperation::Click {
                selector: &command.selector,
                action_name: "press",
            },
            ElementAction::Focus => PollOperation::Focus {
                selector: &command.selector,
            },
            ElementAction::Blur => PollOperation::Blur {
                selector: &command.selector,
            },
        },
    )
    .await
}

async fn run_type(command: TypeCommand) -> CliResult<()> {
    run_pollable(
        &command.target,
        &command.filter,
        command.timeout,
        PollOperation::Type {
            selector: &command.selector,
            text: &command.text,
        },
    )
    .await
}

async fn run_key(command: KeyCommand) -> CliResult<()> {
    run_pollable(
        &command.target,
        &command.filter,
        command.timeout,
        PollOperation::Key {
            selector: &command.selector,
            key: &command.key,
        },
    )
    .await
}

async fn run_mouse_click(command: MouseClickCommand) -> CliResult<()> {
    target::ensure_platform(
        &command.target,
        &[
            PlatformType::MacOS,
            PlatformType::Windows,
            PlatformType::Linux,
        ],
        "mouse-click",
    )?;
    let mut adapter = target::targeted_adapter(&command.target, false).await?;
    operations::tree::mouse_click(
        &mut adapter,
        command.point.x,
        command.point.y,
        command.button.into(),
    )
    .await
}

async fn run_list_windows(command: ListWindowsCommand) -> CliResult<()> {
    target::ensure_platform(
        &command.target,
        &[
            PlatformType::MacOS,
            PlatformType::Windows,
            PlatformType::Linux,
        ],
        "list-windows",
    )?;
    let adapter = target::targeted_adapter(&command.target, false).await?;
    operations::tree::list_windows(&adapter, &command.output).await
}

async fn run_screenshot(command: ScreenshotSubcommand) -> CliResult<()> {
    match command {
        ScreenshotSubcommand::Screen(command) => {
            let adapter = target::targeted_adapter(&command.target, false).await?;
            operations::screenshot::screen(&adapter, &command.overlay).await
        }
        ScreenshotSubcommand::Elements(command) => run_screenshot_elements(command).await,
        ScreenshotSubcommand::Annotate(command) => {
            let mut adapter = target::targeted_adapter(&command.target, true).await?;
            let filter = command.filter.to_tree_filter();
            let tree = get_tree(&mut adapter, &filter).await?;
            operations::screenshot::annotate(&adapter, &tree, &command).await
        }
    }
}

async fn run_screenshot_elements(command: ScreenshotElementsCommand) -> CliResult<()> {
    let mut adapter = target::targeted_adapter(&command.target, true).await?;
    let filter = command.filter.to_tree_filter();
    let tree = get_tree(&mut adapter, &filter).await?;
    operations::screenshot::elements(&adapter, &tree, command.selector.as_deref()).await
}

async fn run_pollable(
    target_args: &TargetArgs,
    filter_args: &TreeFilterArgs,
    timeout: TimeoutArgs,
    operation: PollOperation<'_>,
) -> CliResult<()> {
    let mut adapter = target::targeted_adapter(target_args, true).await?;
    let filter = filter_args.to_tree_filter();

    if timeout.should_poll() {
        run_polling(&mut adapter, &filter, timeout, operation).await
    } else {
        let tree = get_tree(&mut adapter, &filter).await?;
        handle_operation_result(
            execute_poll_operation(&mut adapter, &tree, operation).await?,
            false,
        )
    }
}

async fn run_polling(
    adapter: &mut TargetedAccessibility,
    filter: &TreeFilter,
    timeout: TimeoutArgs,
    operation: PollOperation<'_>,
) -> CliResult<()> {
    let start = std::time::Instant::now();

    loop {
        adapter.clear_cache();
        let tree = match adapter.get_tree(filter).await {
            Ok(tree) => tree,
            Err(e) => {
                let elapsed = start.elapsed().as_millis() as u64;
                if elapsed >= timeout.timeout {
                    return Err(CliError::runtime(format!(
                        "Failed to get accessibility tree after {elapsed}ms: {e}"
                    )));
                }
                tokio::time::sleep(std::time::Duration::from_millis(timeout.poll_interval)).await;
                continue;
            }
        };

        let result = execute_poll_operation(adapter, &tree, operation).await?;
        match result {
            OperationResult::Success => return Ok(()),
            OperationResult::NotFound(msg) => {
                let elapsed = start.elapsed().as_millis() as u64;
                if elapsed >= timeout.timeout {
                    return Err(CliError::runtime(format!(
                        "Timeout after {elapsed}ms: {msg}"
                    )));
                }
                tokio::time::sleep(std::time::Duration::from_millis(timeout.poll_interval)).await;
            }
        }
    }
}

async fn execute_poll_operation(
    adapter: &mut TargetedAccessibility,
    tree: &ElementTree,
    operation: PollOperation<'_>,
) -> CliResult<OperationResult> {
    match operation {
        PollOperation::Query { selector, output } => {
            operations::tree::query(adapter, tree, selector, output)
        }
        PollOperation::Click {
            selector,
            action_name,
        } => operations::tree::click(adapter, tree, selector, action_name).await,
        PollOperation::Focus { selector } => operations::tree::focus(adapter, tree, selector).await,
        PollOperation::Blur { selector } => operations::tree::blur(adapter, tree, selector).await,
        PollOperation::Type { selector, text } => {
            operations::tree::type_value(adapter, tree, selector, text).await
        }
        PollOperation::Key { selector, key } => {
            operations::tree::key(adapter, tree, selector, key).await
        }
    }
}

fn handle_operation_result(result: OperationResult, polling: bool) -> CliResult<()> {
    match result {
        OperationResult::Success => Ok(()),
        OperationResult::NotFound(message) if polling => Err(CliError::runtime(message)),
        OperationResult::NotFound(message) => {
            println!("{message}");
            Ok(())
        }
    }
}

async fn get_tree(
    adapter: &mut TargetedAccessibility,
    filter: &TreeFilter,
) -> CliResult<ElementTree> {
    adapter
        .get_tree(filter)
        .await
        .map_err(|e| CliError::runtime(format!("Failed to get accessibility tree: {e}")))
}

#[derive(Clone, Copy)]
enum ElementAction {
    Click,
    Press,
    Focus,
    Blur,
}

#[derive(Clone, Copy)]
enum PollOperation<'a> {
    Query {
        selector: &'a str,
        output: &'a OutputArgs,
    },
    Click {
        selector: &'a str,
        action_name: &'a str,
    },
    Focus {
        selector: &'a str,
    },
    Blur {
        selector: &'a str,
    },
    Type {
        selector: &'a str,
        text: &'a str,
    },
    Key {
        selector: &'a str,
        key: &'a str,
    },
}
