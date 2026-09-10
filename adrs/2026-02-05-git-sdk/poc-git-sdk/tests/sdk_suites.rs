use std::fs;

use poc_git_sdk::git_cli_sdk::GitCliSdk;
use poc_git_sdk::git_sdk::DummyGitSdk;
use poc_git_sdk::git_sdk::{AuthorInfo, CommitRange, GitSdk, StatusEntry, StatusKind};
use poc_git_sdk::git2_sdk::Git2Sdk;
use poc_git_sdk::gix_sdk::{GixNativeSdk, GixSdk};
use poc_git_sdk::test_repo::TestRepo;

// Shared SDK test cases used by the per-SDK suites below.
#[allow(dead_code)]
mod cases {
    use super::*;

    pub fn dummy_smoke_test(sdk: &dyn GitSdk) {
        let repo = TestRepo::new().expect("failed to create test repo");
        sdk.init(repo.path()).expect("init failed");
        let status = sdk.status(repo.path()).expect("status failed");
        assert!(status.is_empty());
    }

    pub fn init_creates_repo(sdk: &dyn GitSdk) {
        let repo = TestRepo::new().expect("failed to create test repo");
        sdk.init(repo.path()).expect("init failed");
        assert!(repo.path().join(".git").exists());
    }

    pub fn add_and_status_reports_entries(sdk: &dyn GitSdk) {
        let repo = TestRepo::new().expect("failed to create test repo");
        sdk.init(repo.path()).expect("init failed");

        let file_path = repo.path().join("file.txt");
        fs::write(&file_path, "hello").expect("write failed");

        let status = sdk.status(repo.path()).expect("status failed");
        assert_eq!(
            expect_status(&status, "file.txt"),
            Some(StatusKind::Untracked)
        );

        sdk.add(repo.path(), &["file.txt".to_string()])
            .expect("add failed");
        let status = sdk.status(repo.path()).expect("status failed");
        assert_eq!(expect_status(&status, "file.txt"), Some(StatusKind::Added));
    }

    pub fn commit_creates_log_entries(sdk: &dyn GitSdk) {
        let repo = TestRepo::new().expect("failed to create test repo");
        sdk.init(repo.path()).expect("init failed");

        let file_path = repo.path().join("file.txt");
        fs::write(&file_path, "hello").expect("write failed");

        sdk.add(repo.path(), &["file.txt".to_string()])
            .expect("add failed");

        let commit_id = sdk
            .commit(repo.path(), "Initial commit", &default_author())
            .expect("commit failed");
        assert!(!commit_id.is_empty());

        let log = sdk.log(repo.path(), 10).expect("log failed");
        assert!(!log.is_empty());
        assert_eq!(log[0].message, "Initial commit");
    }

    pub fn branch_and_checkout_workflow(sdk: &dyn GitSdk) {
        let repo = TestRepo::new().expect("failed to create test repo");
        sdk.init(repo.path()).expect("init failed");

        let file_path = repo.path().join("file.txt");
        fs::write(&file_path, "main").expect("write failed");
        sdk.add(repo.path(), &["file.txt".to_string()])
            .expect("add failed");
        sdk.commit(repo.path(), "main", &default_author())
            .expect("commit failed");

        sdk.branch(repo.path(), "feature", None)
            .expect("branch failed");
        sdk.checkout(repo.path(), "feature")
            .expect("checkout failed");

        fs::write(&file_path, "feature").expect("write failed");
        sdk.add(repo.path(), &["file.txt".to_string()])
            .expect("add failed");
        sdk.commit(repo.path(), "feature", &default_author())
            .expect("commit failed");

        sdk.checkout(repo.path(), "main")
            .expect("checkout main failed");
    }

    pub fn squash_combines_commit_range(sdk: &dyn GitSdk) {
        let repo = TestRepo::new().expect("failed to create test repo");
        sdk.init(repo.path()).expect("init failed");

        let file_path = repo.path().join("file.txt");

        fs::write(&file_path, "one").expect("write failed");
        sdk.add(repo.path(), &["file.txt".to_string()])
            .expect("add failed");
        sdk.commit(repo.path(), "one", &default_author())
            .expect("commit failed");

        fs::write(&file_path, "two").expect("write failed");
        sdk.add(repo.path(), &["file.txt".to_string()])
            .expect("add failed");
        let start_commit = sdk
            .commit(repo.path(), "two", &default_author())
            .expect("commit failed");

        fs::write(&file_path, "three").expect("write failed");
        sdk.add(repo.path(), &["file.txt".to_string()])
            .expect("add failed");
        let end_commit = sdk
            .commit(repo.path(), "three", &default_author())
            .expect("commit failed");

        let log_before = sdk.log(repo.path(), 10).expect("log failed");
        assert!(log_before.len() >= 3);

        let range = CommitRange {
            start: start_commit,
            end: end_commit,
        };

        sdk.squash(repo.path(), &range, "squashed", &default_author())
            .expect("squash failed");

        let log_after = sdk.log(repo.path(), 10).expect("log failed");
        assert!(log_after.len() + 1 == log_before.len());
        assert_eq!(log_after[0].message, "squashed");
    }

    pub fn diff_and_apply_patch_flow(sdk: &dyn GitSdk) {
        let repo = TestRepo::new().expect("failed to create test repo");
        sdk.init(repo.path()).expect("init failed");

        let file_path = repo.path().join("file.txt");
        fs::write(&file_path, "base").expect("write failed");
        sdk.add(repo.path(), &["file.txt".to_string()])
            .expect("add failed");
        let base_commit = sdk
            .commit(repo.path(), "base", &default_author())
            .expect("commit failed");

        fs::write(&file_path, "change").expect("write failed");
        sdk.add(repo.path(), &["file.txt".to_string()])
            .expect("add failed");
        let change_commit = sdk
            .commit(repo.path(), "change", &default_author())
            .expect("commit failed");

        let patch = sdk
            .diff(repo.path(), Some(&base_commit), Some(&change_commit))
            .expect("diff failed");
        assert!(!patch.is_empty());

        let target_repo = TestRepo::new().expect("failed to create target repo");
        sdk.init(target_repo.path()).expect("init failed");
        let target_path = target_repo.path().join("file.txt");
        fs::write(&target_path, "base").expect("write failed");
        sdk.add(target_repo.path(), &["file.txt".to_string()])
            .expect("add failed");
        sdk.commit(target_repo.path(), "base", &default_author())
            .expect("commit failed");

        sdk.apply_patch(target_repo.path(), &patch)
            .expect("apply patch failed");
        let updated = fs::read_to_string(&target_path).expect("read failed");
        assert_eq!(updated, "change");
    }

    pub fn edge_cases_invalid_input_and_corruption(sdk: &dyn GitSdk) {
        let repo = TestRepo::new().expect("failed to create test repo");
        sdk.init(repo.path()).expect("init failed");

        let add_result = sdk.add(repo.path(), &["missing.txt".to_string()]);
        assert!(add_result.is_err());

        let patch_result = sdk.apply_patch(repo.path(), "not a patch");
        assert!(patch_result.is_err());

        fs::remove_dir_all(repo.path().join(".git")).expect("remove .git failed");
        let status_result = sdk.status(repo.path());
        assert!(status_result.is_err());
    }

    pub fn edge_cases_empty_repo_log(sdk: &dyn GitSdk) {
        let repo = TestRepo::new().expect("failed to create test repo");
        sdk.init(repo.path()).expect("init failed");

        let log = sdk.log(repo.path(), 10).expect("log failed");
        assert!(log.is_empty());
    }

    fn default_author() -> AuthorInfo {
        AuthorInfo {
            name: "Test Author".to_string(),
            email: "test@example.com".to_string(),
        }
    }

    fn expect_status(statuses: &[StatusEntry], path: &str) -> Option<StatusKind> {
        statuses
            .iter()
            .find(|entry| entry.path == path)
            .map(|entry| entry.status)
    }
}

macro_rules! sdk_test_suite {
    ($sdk:expr, smoke) => {
        #[test]
        fn dummy_smoke_test() {
            let sdk = $sdk;
            crate::cases::dummy_smoke_test(&sdk);
        }
    };
    ($sdk:expr, full) => {
        #[test]
        fn init_creates_repo() {
            let sdk = $sdk;
            crate::cases::init_creates_repo(&sdk);
        }

        #[test]
        fn add_and_status_reports_entries() {
            let sdk = $sdk;
            crate::cases::add_and_status_reports_entries(&sdk);
        }

        #[test]
        fn commit_creates_log_entries() {
            let sdk = $sdk;
            crate::cases::commit_creates_log_entries(&sdk);
        }

        #[test]
        fn branch_and_checkout_workflow() {
            let sdk = $sdk;
            crate::cases::branch_and_checkout_workflow(&sdk);
        }

        #[test]
        fn squash_combines_commit_range() {
            let sdk = $sdk;
            crate::cases::squash_combines_commit_range(&sdk);
        }

        #[test]
        fn diff_and_apply_patch_flow() {
            let sdk = $sdk;
            crate::cases::diff_and_apply_patch_flow(&sdk);
        }

        #[test]
        fn edge_cases_invalid_input_and_corruption() {
            let sdk = $sdk;
            crate::cases::edge_cases_invalid_input_and_corruption(&sdk);
        }

        #[test]
        fn edge_cases_empty_repo_log() {
            let sdk = $sdk;
            crate::cases::edge_cases_empty_repo_log(&sdk);
        }
    };
    ($sdk:expr, ignored) => {
        #[test]
        #[ignore]
        fn init_creates_repo() {
            let sdk = $sdk;
            crate::cases::init_creates_repo(&sdk);
        }

        #[test]
        #[ignore]
        fn add_and_status_reports_entries() {
            let sdk = $sdk;
            crate::cases::add_and_status_reports_entries(&sdk);
        }

        #[test]
        #[ignore]
        fn commit_creates_log_entries() {
            let sdk = $sdk;
            crate::cases::commit_creates_log_entries(&sdk);
        }

        #[test]
        #[ignore]
        fn branch_and_checkout_workflow() {
            let sdk = $sdk;
            crate::cases::branch_and_checkout_workflow(&sdk);
        }

        #[test]
        #[ignore]
        fn squash_combines_commit_range() {
            let sdk = $sdk;
            crate::cases::squash_combines_commit_range(&sdk);
        }

        #[test]
        #[ignore]
        fn diff_and_apply_patch_flow() {
            let sdk = $sdk;
            crate::cases::diff_and_apply_patch_flow(&sdk);
        }

        #[test]
        #[ignore]
        fn edge_cases_invalid_input_and_corruption() {
            let sdk = $sdk;
            crate::cases::edge_cases_invalid_input_and_corruption(&sdk);
        }

        #[test]
        #[ignore]
        fn edge_cases_empty_repo_log() {
            let sdk = $sdk;
            crate::cases::edge_cases_empty_repo_log(&sdk);
        }
    };
    ($sdk:expr, partial) => {
        #[test]
        fn init_creates_repo() {
            let sdk = $sdk;
            crate::cases::init_creates_repo(&sdk);
        }

        #[test]
        fn edge_cases_empty_repo_log() {
            let sdk = $sdk;
            crate::cases::edge_cases_empty_repo_log(&sdk);
        }
    };
}

mod dummy {
    use super::*;

    sdk_test_suite!(DummyGitSdk, smoke);
}

mod git2 {
    use super::*;

    sdk_test_suite!(Git2Sdk, full);
}

mod git_cli {
    use super::*;

    sdk_test_suite!(GitCliSdk, full);
}

mod gix {
    use super::*;

    sdk_test_suite!(GixSdk, full);
}

mod gix_native {
    use super::*;

    sdk_test_suite!(GixNativeSdk, partial);
}
