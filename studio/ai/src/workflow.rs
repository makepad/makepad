use makepad_studio_protocol::hub_protocol::{ActiveWorkflowState, WorkflowStepState};

#[derive(Clone, Debug)]
pub struct ParsedWorkflow {
    pub name: String,
    pub steps: Vec<WorkflowStep>,
}

#[derive(Clone, Debug)]
pub struct WorkflowStep {
    pub name: String,
    pub description: String,
}

pub fn parse_workflow_markdown(content: &str) -> Option<ParsedWorkflow> {
    let mut name = String::new();
    let mut steps = Vec::new();
    let mut in_steps = false;
    let mut current_step_name = None;
    let mut current_step_desc = Vec::new();

    let lines: Vec<&str> = content.lines().collect();
    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with("# ") {
            if name.is_empty() {
                name = trimmed
                    .strip_prefix("# ")
                    .unwrap_or(trimmed)
                    .trim()
                    .to_string();
            }
        } else if trimmed.starts_with("## ") {
            if trimmed
                .to_lowercase()
                .strip_prefix("##")
                .unwrap_or("")
                .trim()
                == "steps"
            {
                in_steps = true;
            } else {
                in_steps = false;
            }
        } else if in_steps && trimmed.starts_with("### ") {
            if let Some(s_name) = current_step_name.take() {
                let s_desc = current_step_desc.join("\n").trim().to_string();
                steps.push(WorkflowStep {
                    name: s_name,
                    description: s_desc,
                });
                current_step_desc.clear();
            }

            let raw_step_name = trimmed.strip_prefix("###").unwrap_or(trimmed).trim();
            let mut chars = raw_step_name.chars().peekable();
            let mut has_digits = false;
            while let Some(&c) = chars.peek() {
                if c.is_ascii_digit() {
                    has_digits = true;
                    chars.next();
                } else {
                    break;
                }
            }
            let parsed_name = if has_digits && chars.peek() == Some(&'.') {
                chars.next();
                let rest: String = chars.collect();
                rest.trim().to_string()
            } else {
                raw_step_name.to_string()
            };

            current_step_name = Some(parsed_name);
        } else if in_steps && current_step_name.is_some() {
            current_step_desc.push(line);
        }
    }

    if let Some(s_name) = current_step_name {
        let s_desc = current_step_desc.join("\n").trim().to_string();
        steps.push(WorkflowStep {
            name: s_name,
            description: s_desc,
        });
    }

    if name.trim().is_empty() || steps.is_empty() {
        return None;
    }

    Some(ParsedWorkflow { name, steps })
}

pub fn append_workflow_focus(
    out: &mut String,
    active_workflow: &ActiveWorkflowState,
    workflows: &[ParsedWorkflow],
) {
    out.push_str("\n\n# Current Workflow\n");
    out.push_str("Workflow: ");
    out.push_str(active_workflow.name.trim());
    out.push('\n');

    if let Some(step) = active_workflow.steps.get(active_workflow.current_step) {
        out.push_str("Current step: ");
        out.push_str(&(active_workflow.current_step + 1).to_string());
        out.push_str(". ");
        out.push_str(step.name.trim());
        out.push('\n');
        out.push_str("Status: ");
        out.push_str(step.status.trim());
        out.push('\n');

        if let Some(description) =
            workflow_step_description(workflows, &active_workflow.name, &step.name)
        {
            out.push_str("Description:\n");
            out.push_str(description.trim());
            out.push('\n');
        }
    }
}

pub fn workflow_step_description<'a>(
    workflows: &'a [ParsedWorkflow],
    workflow_name: &str,
    step_name: &str,
) -> Option<&'a str> {
    workflows
        .iter()
        .find(|workflow| workflow.name == workflow_name)
        .and_then(|workflow| {
            workflow
                .steps
                .iter()
                .find(|step| step.name == step_name)
                .map(|step| step.description.as_str())
        })
        .filter(|description| !description.trim().is_empty())
}

pub fn workflow_prompt_from_command(
    prompt: &str,
    workflows: &[ParsedWorkflow],
) -> Option<(ActiveWorkflowState, String)> {
    let command_line = prompt.strip_prefix('/')?;
    let (command, arguments) = command_line
        .split_once(char::is_whitespace)
        .map(|(command, arguments)| (command.trim(), arguments.trim()))
        .unwrap_or((command_line.trim(), ""));
    if command.is_empty() {
        return None;
    }
    let workflow = workflows
        .iter()
        .find(|workflow| workflow_command_matches(&workflow.name, command))?;
    let first_step = workflow.steps.first()?;
    let mut steps = Vec::with_capacity(workflow.steps.len());
    for (index, step) in workflow.steps.iter().enumerate() {
        steps.push(WorkflowStepState {
            name: step.name.clone(),
            status: if index == 0 { "active" } else { "pending" }.to_string(),
        });
    }

    let mut instruction = String::new();
    instruction.push_str("Execute workflow `");
    instruction.push_str(&workflow.name);
    instruction.push_str("`.");
    if !arguments.is_empty() {
        instruction.push_str("\nArguments: ");
        instruction.push_str(arguments);
    }
    instruction.push_str("\nFocus on step 1: ");
    instruction.push_str(&first_step.name);
    instruction.push_str("\nStatus: active");
    if !first_step.description.trim().is_empty() {
        instruction.push_str("\nStep description:\n");
        instruction.push_str(first_step.description.trim());
    }
    instruction.push_str("\n\nComplete this step before moving to later workflow steps.");

    Some((
        ActiveWorkflowState {
            name: workflow.name.clone(),
            current_step: 0,
            steps,
        },
        instruction,
    ))
}

pub fn workflow_command_matches(workflow_name: &str, command: &str) -> bool {
    workflow_name == command || workflow_command_slug(workflow_name) == command
}

pub fn workflow_command_slug(name: &str) -> String {
    let mut slug = String::new();
    let mut pending_dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_dash && !slug.is_empty() {
                slug.push('-');
            }
            slug.push(ch.to_ascii_lowercase());
            pending_dash = false;
        } else {
            pending_dash = true;
        }
    }
    slug
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_workflow_markdown() {
        let content = r#"# Review PRs Command

Some intro text...

## Steps
### 1. Resolve PR Set
Description of step 1...
Detailed instructions...

### 2. Verify Changes
Description of step 2...

## Feedback
Not steps.
"#;
        let parsed = parse_workflow_markdown(content).unwrap();
        assert_eq!(parsed.name, "Review PRs Command");
        assert_eq!(parsed.steps.len(), 2);
        assert_eq!(parsed.steps[0].name, "Resolve PR Set");
        assert_eq!(
            parsed.steps[0].description,
            "Description of step 1...\nDetailed instructions..."
        );
        assert_eq!(parsed.steps[1].name, "Verify Changes");
        assert_eq!(parsed.steps[1].description, "Description of step 2...");
    }

    #[test]
    fn test_parse_workflow_markdown_validation() {
        let content_no_name = r#"
## Steps
### 1. Resolve PR Set
Description of step 1...
"#;
        assert!(parse_workflow_markdown(content_no_name).is_none());

        let content_no_steps = r#"# Review PRs Command

Some intro text...

## Steps
"#;
        assert!(parse_workflow_markdown(content_no_steps).is_none());
    }

    #[test]
    fn workflow_command_slug_normalizes_names() {
        assert_eq!(
            workflow_command_slug("Review PRs Command"),
            "review-prs-command"
        );
        assert_eq!(workflow_command_slug("  Ship: v2! "), "ship-v2");
    }
}
