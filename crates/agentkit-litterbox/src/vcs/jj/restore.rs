use crate::vcs::{Restored, Revision, Vcs};

use super::{Error, JjVcs, ops};

/// Add one commit on the shared branch whose tree is `target`, then move the
/// sandbox working copy and `synced` to it.
pub(super) fn restore(
    vcs: &JjVcs,
    workspace: &crate::vcs::Workspace,
    sandbox: &str,
    target: &Revision,
) -> Result<Restored, Error> {
    let path = vcs.workspace(sandbox);
    if !path.is_dir() {
        return Err(Error::Backend {
            operation: "restore".to_string(),
            message: format!(
                "the jj workspace for '{sandbox}' is absent; repair the sandbox first"
            ),
        });
    }

    let tip = vcs.tip(workspace)?;
    if !vcs.conflicts(&tip)?.is_empty() {
        return Err(Error::Backend {
            operation: "restore".to_string(),
            message: "the shared commit is conflicted; resolve it first".to_string(),
        });
    }

    let target_text = target.as_str();
    let target_tree = vcs.tree(target)?;
    let tip_tree = vcs.tree(&tip)?;
    let sandbox_tip = vcs.sandbox_tip(workspace)?;
    let sandbox_tree = vcs.tree(&sandbox_tip)?;

    if tip_tree == target_tree
        && sandbox_tree == target_tree
        && vcs.is_ancestor(&tip, &sandbox_tip)?
    {
        vcs.set_synced(workspace, &tip)?;
        return Ok(Restored {
            revision: tip,
            created: false,
        });
    }

    let message = format!(
        "litterbox: restore {} to {}",
        workspace.branch().as_str(),
        target_text
    );
    ops::jj_ok(&path, &["new", tip.as_str(), "-m", &message])?;
    ops::jj_ok(&path, &["restore", "--from", target_text])?;
    let commit = ops::commit_id(&path, "@")?;
    // Publish with the expected tip. An external push since the read above
    // must surface as a race rather than being overwritten by a commit
    // parented on the old tip.
    let reference = format!("refs/heads/{}", workspace.branch().as_str());
    ops::git_update_ref(&vcs.root, &reference, &commit, tip.as_str())?;
    ops::jj_ok(&vcs.root, &["git", "import"])?;
    // The restore commit is now both the shared and the sandbox tip. Start a
    // fresh child so editing the workspace cannot rewrite either.
    vcs.write_sandbox(sandbox, &commit)?;
    ops::jj_ok(&path, &["new", commit.as_str()])?;
    vcs.write_synced(sandbox, &commit)?;

    Ok(Restored {
        revision: Revision::new(commit),
        created: true,
    })
}
