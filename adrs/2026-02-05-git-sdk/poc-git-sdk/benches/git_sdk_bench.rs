use criterion::{Criterion, criterion_group, criterion_main};

use poc_git_sdk::git_sdk::{AuthorInfo, DummyGitSdk, GitSdk};
use poc_git_sdk::test_repo::TestRepo;

fn default_author() -> AuthorInfo {
    AuthorInfo {
        name: "Bench Author".to_string(),
        email: "bench@example.com".to_string(),
    }
}

fn bench_dummy_sdk(c: &mut Criterion) {
    let sdk = DummyGitSdk;
    let repo = TestRepo::new().expect("failed to create test repo");
    sdk.init(repo.path()).expect("init failed");

    c.bench_function("dummy_status", |b| {
        b.iter(|| {
            let _ = sdk.status(repo.path());
        })
    });

    c.bench_function("dummy_commit", |b| {
        b.iter(|| {
            let _ = sdk.commit(repo.path(), "bench", &default_author());
        })
    });
}

criterion_group!(benches, bench_dummy_sdk);
criterion_main!(benches);
