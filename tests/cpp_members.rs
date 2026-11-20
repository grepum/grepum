//! C++ calls through members used without `this->` (Unreal style), smart
//! pointers, inherited members and class-scoped type aliases. Regression for
//! a real miss: `State->Rides->show(...)` reported `(no callers)`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn setup() -> PathBuf {
    let root = std::env::temp_dir().join(format!("grepum-cpp-members-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    write(&root, "lib/ride.hpp", "#pragma once\nnamespace game::sound {\nclass RideLoops {\npublic:\n    void show(float rate);\n    void hide();\n    void stop_all();\n};\n}\n");
    write(&root, "lib/ride.cpp", "#include \"ride.hpp\"\nnamespace game::sound {\nvoid RideLoops::show(float rate) {}\nvoid RideLoops::hide() {}\nvoid RideLoops::stop_all() {}\n}\n");
    write(&root, "lib/core.hpp", "#pragma once\nnamespace parts {\nstruct JobCore {\n    void drop_run(int id) {}\n};\n}\n");
    write(&root, "Source/Game/Public/SoundSubsystem.h", "#pragma once\nstruct FSoundState;\nclass USoundBase {\nprotected:\n    TSharedPtr<FSoundState> Shared;\n};\nUCLASS()\nclass GAME_API USoundSubsystem : public USoundBase {\n    GENERATED_BODY()\npublic:\n    void Tick(float Dt);\n    void Stop();\n    void Direct();\n    void FromBase();\nprivate:\n    TUniquePtr<FSoundState> State;\n};\n");
    write(&root, "Source/Game/Private/SoundSubsystem.cpp", "#include \"SoundSubsystem.h\"\n#include \"ride.hpp\"\nstruct FSoundState {\n    TUniquePtr<game::sound::RideLoops> Rides;\n    game::sound::RideLoops Plain;\n};\nvoid USoundSubsystem::Tick(float Dt) {\n    State->Rides->show(Dt);\n}\nvoid USoundSubsystem::Stop() {\n    State->Rides->hide();\n}\nvoid USoundSubsystem::Direct() {\n    State->Plain.show(1.0f);\n}\nvoid USoundSubsystem::FromBase() {\n    Shared->Rides->hide();\n}\n");
    write(&root, "lib/world.hpp", "#pragma once\n#include \"core.hpp\"\nclass World {\npublic:\n    void apply(int id) { core_.drop_run(id); }\nprivate:\n    using Core = parts::JobCore;\n    Core core_;\n};\n");
    for args in [&["init", "-q"][..], &["add", "-A"], &["-c", "user.name=t", "-c", "user.email=t@e", "commit", "-qm", "i"]] {
        assert!(Command::new("git").args(args).current_dir(&root).status().unwrap().success());
    }
    root
}

fn callers(root: &Path, sym: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_grepum")).args(["callers", sym]).current_dir(root).output().unwrap();
    String::from_utf8_lossy(&out.stdout).to_string()
}

#[test]
fn bare_members_smart_pointers_bases_and_aliases() {
    let root = setup();
    let show = callers(&root, "RideLoops.show");
    assert!(show.contains("<- USoundSubsystem.Tick"), "TUniquePtr member chain:\n{show}");
    assert!(show.contains("<- USoundSubsystem.Direct"), "plain member:\n{show}");
    let hide = callers(&root, "RideLoops.hide");
    assert!(hide.contains("<- USoundSubsystem.Stop"), "{hide}");
    assert!(hide.contains("<- USoundSubsystem.FromBase"), "member inherited from a base class:\n{hide}");
    let drop = callers(&root, "JobCore.drop_run");
    assert!(drop.contains("<- World.apply"), "class-scoped alias:\n{drop}");
    let none = callers(&root, "RideLoops.stop_all");
    assert!(none.contains("no callers found") && none.contains("rg -w stop_all"), "{none}");
    let _ = std::fs::remove_dir_all(&root);
}
