"""Unreal Engine 5 style C++ project generator (used by generate.py)."""
from __future__ import annotations

import json
import shutil

WEAPONS = ["Rifle", "Shotgun", "Pistol", "Smg", "Sniper", "RocketLauncher", "GrenadeLauncher", "Flamethrower",
           "Railgun", "Crossbow", "Minigun", "PlasmaRifle", "LaserCannon", "Revolver", "BurstRifle", "Carbine",
           "Blaster", "Harpoon", "NailGun", "Taser", "FreezeRay", "Shockwave", "Boomstick", "Musket", "Slingshot"]
ENEMIES = ["Zombie", "Skeleton", "Ghoul", "Wraith", "Golem", "Imp", "Drone", "Sentinel", "Brute", "Stalker",
           "Spitter", "Crawler", "Banshee", "Reaper", "Hound", "Mauler", "Warden", "Shade", "Titan", "Lurker"]
PICKUPS = ["HealthPack", "Medkit", "Bandage", "Stim", "Adrenaline", "ArmorShard", "MegaHealth", "Regen", "Potion",
           "Elixir", "Ration", "Tonic", "Salve", "Herb", "Antidote", "Serum", "Booster", "Cell", "Orb", "Relic"]


def gen_arena(ROOT, Truth, Src, n_weapons: int) -> None:
    root = ROOT / "projects" / "arena"
    shutil.rmtree(root, ignore_errors=True)
    T = Truth()
    files = []
    PUB, PRI = "Source/Arena/Public", "Source/Arena/Private"

    def src(rel):
        s = Src(root, rel)
        files.append(s)
        return s

    class Cls:
        """One UCLASS: header + cpp, tracking prototype and definition lines."""

        def __init__(self, name, h, c):
            self.name, self.h, self.c, self.decls, self.defs = name, h, c, {}, {}

        def decl(self, line, meth, macro=None):
            if macro:
                self.h.add(f"\t{macro}")
            l = self.h.add(f"\t{line}")
            self.h.add("")
            key = T.define(self.h.rel, f"{self.name}.{meth}#decl", l, "decl")
            self.decls[meth] = key
            return key

        def inline(self, line, meth, macro=None):
            if macro:
                self.h.add(f"\t{macro}")
            l = self.h.add(f"\t{line}")
            self.h.add("")
            key = T.define(self.h.rel, f"{self.name}.{meth}", l, "method")
            self.defs[meth] = key
            return key

        def define(self, sig, meth, body):
            l = self.c.add(sig, "{", *[f"\t{b}" for b in body], "}", "")
            key = T.define(self.c.rel, f"{self.name}.{meth}", l, "method")
            self.defs[meth] = key
            if meth in self.decls:
                T.aliases.append((self.decls[meth], key))
            return key

    def header(rel, includes, fwd=()):
        h = src(rel)
        h.add("// Copyright Arena Games. All Rights Reserved.", "", "#pragma once", "", '#include "CoreMinimal.h"')
        for i in includes:
            h.add(f'#include "{i}"')
        h.add(f'#include "{rel.rsplit("/", 1)[-1][:-2]}.generated.h"', "")
        for f_ in fwd:
            h.add(f"class {f_};")
        if fwd:
            h.add("")
        return h

    def cpp(rel, includes):
        c = src(rel)
        c.add("// Copyright Arena Games. All Rights Reserved.", "")
        for i in includes:
            c.add(f'#include "{i}"')
        c.add("")
        return c

    def uclass(h, macro, name, base):
        h.add(macro)
        T.define(h.rel, name, h.add(f"class ARENA_API {name} : public {base}"), "class")
        h.add("{", "\tGENERATED_BODY()", "")

    # ---- project files (not indexed, but real UE projects have them)
    (root / "Source" / "Arena").mkdir(parents=True, exist_ok=True)
    (root / "Arena.uproject").write_text(json.dumps({"FileVersion": 3, "EngineAssociation": "5.4", "Modules": [
        {"Name": "Arena", "Type": "Runtime", "LoadingPhase": "Default"}]}, indent=1) + "\n")
    (root / "Source" / "Arena" / "Arena.Build.cs").write_text(
        "using UnrealBuildTool;\n\npublic class Arena : ModuleRules\n{\n\tpublic Arena(ReadOnlyTargetRules Target) : base(Target)\n\t{\n"
        "\t\tPCHUsage = PCHUsageMode.UseExplicitOrSharedPCHs;\n"
        '\t\tPublicDependencyModuleNames.AddRange(new string[] { "Core", "CoreUObject", "Engine", "InputCore", "EnhancedInput" });\n'
        "\t}\n}\n")

    c = cpp(f"{PRI}/Arena.cpp", ["Modules/ModuleManager.h", "ArenaTypes.h"])
    c.add('IMPLEMENT_PRIMARY_GAME_MODULE(FDefaultGameModuleImpl, Arena, "Arena");', "", "DEFINE_LOG_CATEGORY(LogArena);")

    # ---- ArenaTypes.h: a USTRUCT and delegate types
    h = header(f"{PUB}/ArenaTypes.h", [])
    h.add("DECLARE_LOG_CATEGORY_EXTERN(LogArena, Log, All);", "")
    l = h.add("USTRUCT(BlueprintType)", "struct FDamageInfo", "{", "\tGENERATED_BODY()", "",
              "\tUPROPERTY(BlueprintReadWrite)", "\tfloat Amount = 0.f;", "",
              "\tUPROPERTY(BlueprintReadWrite)", "\tfloat Multiplier = 1.f;", "",
              "\tUPROPERTY(BlueprintReadWrite)", "\tFVector Direction = FVector::ZeroVector;", "};", "")
    T.define(h.rel, "FDamageInfo", l + 1, "type")
    h.add("DECLARE_DYNAMIC_MULTICAST_DELEGATE_TwoParams(FOnHealthChanged, float, NewHealth, float, Delta);",
          "DECLARE_DYNAMIC_MULTICAST_DELEGATE_OneParam(FOnScoreChanged, int32, NewScore);")

    # ---- ArenaDamage: free functions in a namespace
    h = src(f"{PUB}/ArenaDamage.h")
    h.add("#pragma once", "", '#include "CoreMinimal.h"', '#include "ArenaTypes.h"', "", "namespace ArenaDamage", "{")
    DD = {}
    for name, sig in [("MakeDamage", "FDamageInfo MakeDamage(float Base, const FVector& Direction);"),
                      ("Falloff", "float Falloff(float Distance, float Range);"),
                      ("Scale", "float Scale(float Amount, float Multiplier);")]:
        DD[name] = T.define(h.rel, f"{name}#decl", h.add(f"\tARENA_API {sig}"), "decl")
    h.add("}")
    c = cpp(f"{PRI}/ArenaDamage.cpp", ["ArenaDamage.h"])
    c.add("namespace ArenaDamage", "{")
    l = c.add("\tFDamageInfo MakeDamage(float Base, const FVector& Direction)", "\t{", "\t\tFDamageInfo Info;",
              "\t\tInfo.Amount = Scale(Base, 1.f);", "\t\tInfo.Direction = Direction;", "\t\treturn Info;", "\t}", "")
    MAKEDMG = T.define(c.rel, "MakeDamage", l, "fn")
    l = c.add("\tfloat Falloff(float Distance, float Range)", "\t{", "\t\treturn FMath::Clamp(1.f - Distance / Range, 0.f, 1.f);", "\t}", "")
    FALLOFF = T.define(c.rel, "Falloff", l, "fn")
    l = c.add("\tfloat Scale(float Amount, float Multiplier)", "\t{", "\t\treturn Amount * Multiplier;", "\t}")
    SCALE = T.define(c.rel, "Scale", l, "fn")
    c.add("}")
    T.call(MAKEDMG, SCALE)
    for k, v in [("MakeDamage", MAKEDMG), ("Falloff", FALLOFF), ("Scale", SCALE)]:
        T.aliases.append((DD[k], v))

    # ---- UHealthComponent
    h = header(f"{PUB}/Components/HealthComponent.h", ["Components/ActorComponent.h", "ArenaTypes.h"])
    c = cpp(f"{PRI}/Components/HealthComponent.cpp", ["Components/HealthComponent.h", "ArenaDamage.h", "Net/UnrealNetwork.h"])
    HC = Cls("UHealthComponent", h, c)
    uclass(h, "UCLASS(ClassGroup = (Arena), meta = (BlueprintSpawnableComponent))", "UHealthComponent", "UActorComponent")
    h.add("public:", "\tUHealthComponent();", "")
    HC.decl("void ApplyDamage(const FDamageInfo& Info);", "ApplyDamage", 'UFUNCTION(BlueprintCallable, Category = "Health")')
    HC.decl("void Heal(float Amount);", "Heal", 'UFUNCTION(BlueprintCallable, Category = "Health")')
    HC.decl("bool IsDead() const;", "IsDead", 'UFUNCTION(BlueprintPure, Category = "Health")')
    h.add("\tUPROPERTY(BlueprintAssignable)", "\tFOnHealthChanged OnHealthChanged;", "", "protected:")
    HC.decl("void OnRep_Health(float OldHealth);", "OnRep_Health", "UFUNCTION()")
    HC.decl("float ClampHealth(float Value) const;", "ClampHealth")
    h.add('\tUPROPERTY(ReplicatedUsing = OnRep_Health, EditAnywhere, Category = "Health")', "\tfloat Health = 100.f;", "",
          "\tUPROPERTY(EditAnywhere)", "\tfloat MaxHealth = 100.f;", "};")
    c.add("UHealthComponent::UHealthComponent()", "{", "\tPrimaryComponentTick.bCanEverTick = false;", "\tSetIsReplicatedByDefault(true);", "}", "")
    HC.define("void UHealthComponent::ApplyDamage(const FDamageInfo& Info)", "ApplyDamage", [
        "const float Old = Health;", "Health = ClampHealth(Health - ArenaDamage::Scale(Info.Amount, Info.Multiplier));",
        "OnHealthChanged.Broadcast(Health, Health - Old);", "if (IsDead())", "{",
        '\tUE_LOG(LogArena, Log, TEXT("%s died"), *GetNameSafe(GetOwner()));', "}"])
    HC.define("void UHealthComponent::Heal(float Amount)", "Heal", [
        "const float Old = Health;", "Health = ClampHealth(Health + Amount);", "OnHealthChanged.Broadcast(Health, Health - Old);"])
    HC.define("bool UHealthComponent::IsDead() const", "IsDead", ["return Health <= 0.f;"])
    HC.define("void UHealthComponent::OnRep_Health(float OldHealth)", "OnRep_Health", ["OnHealthChanged.Broadcast(Health, Health - OldHealth);"])
    HC.define("float UHealthComponent::ClampHealth(float Value) const", "ClampHealth", ["return FMath::Clamp(Value, 0.f, MaxHealth);"])
    for t in (HC.defs["ClampHealth"], SCALE, HC.defs["IsDead"]):
        T.call(HC.defs["ApplyDamage"], t)
    T.call(HC.defs["Heal"], HC.defs["ClampHealth"])

    # ---- UScoreSubsystem
    h = header(f"{PUB}/Subsystems/ScoreSubsystem.h", ["Subsystems/GameInstanceSubsystem.h", "ArenaTypes.h"])
    c = cpp(f"{PRI}/Subsystems/ScoreSubsystem.cpp", ["Subsystems/ScoreSubsystem.h"])
    SS = Cls("UScoreSubsystem", h, c)
    uclass(h, "UCLASS()", "UScoreSubsystem", "UGameInstanceSubsystem")
    h.add("public:")
    SS.decl("void AddKill(AActor* Victim);", "AddKill", "UFUNCTION(BlueprintCallable)")
    SS.decl("void AddBonus(int32 Points);", "AddBonus", "UFUNCTION(BlueprintCallable)")
    SS.decl("void StartRound(int32 Round);", "StartRound", "UFUNCTION(BlueprintCallable)")
    SS.inline("int32 GetScore() const { return Score; }", "GetScore", "UFUNCTION(BlueprintPure)")
    h.add("\tUPROPERTY(BlueprintAssignable)", "\tFOnScoreChanged OnScoreChanged;", "", "private:")
    SS.decl("void BroadcastScore();", "BroadcastScore")
    h.add("\tint32 Score = 0;", "\tint32 CurrentRound = 0;", "};")
    SS.define("void UScoreSubsystem::AddKill(AActor* Victim)", "AddKill", ["Score += 10 * FMath::Max(CurrentRound, 1);", "BroadcastScore();"])
    SS.define("void UScoreSubsystem::AddBonus(int32 Points)", "AddBonus", ["Score += Points;", "BroadcastScore();"])
    SS.define("void UScoreSubsystem::StartRound(int32 Round)", "StartRound", ["CurrentRound = Round;", "BroadcastScore();"])
    SS.define("void UScoreSubsystem::BroadcastScore()", "BroadcastScore", ["OnScoreChanged.Broadcast(GetScore());"])
    for m in ("AddKill", "AddBonus", "StartRound"):
        T.call(SS.defs[m], SS.defs["BroadcastScore"])
    T.call(SS.defs["BroadcastScore"], SS.defs["GetScore"])

    # ---- UArenaStatics: Blueprint function library (static calls)
    h = header(f"{PUB}/ArenaStatics.h", ["Kismet/BlueprintFunctionLibrary.h"], ["UScoreSubsystem"])
    c = cpp(f"{PRI}/ArenaStatics.cpp", ["ArenaStatics.h", "Subsystems/ScoreSubsystem.h", "Kismet/GameplayStatics.h"])
    ST = Cls("UArenaStatics", h, c)
    uclass(h, "UCLASS()", "UArenaStatics", "UBlueprintFunctionLibrary")
    h.add("public:")
    ST.decl("static UScoreSubsystem* GetScoreSubsystem(const UObject* WorldContext);", "GetScoreSubsystem",
            'UFUNCTION(BlueprintPure, meta = (WorldContext = "WorldContext"))')
    ST.decl("static AActor* TraceForActor(const AActor* From, const FVector& Direction, float Range);", "TraceForActor")
    h.add("};")
    ST.define("UScoreSubsystem* UArenaStatics::GetScoreSubsystem(const UObject* WorldContext)", "GetScoreSubsystem", [
        "const UGameInstance* GameInstance = UGameplayStatics::GetGameInstance(WorldContext);",
        "return GameInstance ? GameInstance->GetSubsystem<UScoreSubsystem>() : nullptr;"])
    ST.define("AActor* UArenaStatics::TraceForActor(const AActor* From, const FVector& Direction, float Range)", "TraceForActor", [
        "FHitResult Hit;", "const FVector Start = From->GetActorLocation();",
        "From->GetWorld()->LineTraceSingleByChannel(Hit, Start, Start + Direction * Range, ECC_Pawn);", "return Hit.GetActor();"])

    # ---- AArenaWeapon: RPCs and Blueprint events
    h = header(f"{PUB}/Weapons/ArenaWeapon.h", ["GameFramework/Actor.h", "ArenaTypes.h"], ["AArenaCharacter"])
    c = cpp(f"{PRI}/Weapons/ArenaWeapon.cpp", ["Weapons/ArenaWeapon.h", "Characters/ArenaCharacter.h", "Kismet/GameplayStatics.h"])
    W = Cls("AArenaWeapon", h, c)
    uclass(h, "UCLASS(Abstract, Blueprintable)", "AArenaWeapon", "AActor")
    h.add("public:", "\tAArenaWeapon();", "")
    W.decl("void Fire();", "Fire", 'UFUNCTION(BlueprintCallable, Category = "Weapon")')
    W.decl("void ServerFire(const FVector& Aim);", "ServerFire", "UFUNCTION(Server, Reliable, WithValidation)")
    W.decl("void PlayFireEffects();", "PlayFireEffects", 'UFUNCTION(BlueprintNativeEvent, Category = "Weapon")')
    W.decl("void OnAmmoDepleted();", "OnAmmoDepleted", 'UFUNCTION(BlueprintImplementableEvent, Category = "Weapon")')
    W.decl("AArenaCharacter* GetOwningCharacter() const;", "GetOwningCharacter")
    h.add("protected:")
    W.decl("virtual void OnFire(const FVector& Aim);", "OnFire")
    W.decl("bool CanFire() const;", "CanFire")
    h.add('\tUPROPERTY(EditDefaultsOnly, Category = "Weapon")', "\tfloat BaseDamage = 10.f;", "",
          "\tUPROPERTY(Replicated)", "\tint32 Ammo = 30;", "};")
    c.add("AArenaWeapon::AArenaWeapon()", "{", "\tbReplicates = true;", "}", "")
    W.define("void AArenaWeapon::Fire()", "Fire", [
        "if (!CanFire())", "{", "\tOnAmmoDepleted();", "\treturn;", "}", "const FVector Aim = GetActorForwardVector();",
        "if (!HasAuthority())", "{", "\tServerFire(Aim);", "\treturn;", "}", "OnFire(Aim);", "PlayFireEffects();"])
    W.define("void AArenaWeapon::ServerFire_Implementation(const FVector& Aim)", "ServerFire_Implementation", ["OnFire(Aim);"])
    W.define("bool AArenaWeapon::ServerFire_Validate(const FVector& Aim)", "ServerFire_Validate", ["return Ammo > 0;"])
    W.define("void AArenaWeapon::PlayFireEffects_Implementation()", "PlayFireEffects_Implementation",
             ["UGameplayStatics::PlaySoundAtLocation(this, nullptr, GetActorLocation());"])
    W.define("AArenaCharacter* AArenaWeapon::GetOwningCharacter() const", "GetOwningCharacter", ["return Cast<AArenaCharacter>(GetOwner());"])
    W.define("void AArenaWeapon::OnFire(const FVector& Aim)", "OnFire", ["--Ammo;"])
    W.define("bool AArenaWeapon::CanFire() const", "CanFire", ["return Ammo > 0;"])
    F = W.defs["Fire"]
    T.call(F, W.defs["CanFire"])
    T.call(F, W.decls["OnAmmoDepleted"])  # BlueprintImplementableEvent: only the prototype exists in C++
    T.call(F, W.defs["ServerFire_Implementation"], hard="ue-rpc")
    T.call(F, W.defs["OnFire"])
    T.call(F, W.defs["PlayFireEffects_Implementation"], hard="ue-rpc")
    T.call(W.defs["ServerFire_Implementation"], W.defs["OnFire"])

    # ---- AArenaCharacter: components, input and delegate binding
    h = header(f"{PUB}/Characters/ArenaCharacter.h", ["GameFramework/Character.h"], ["UHealthComponent", "AArenaWeapon", "UInputAction"])
    c = cpp(f"{PRI}/Characters/ArenaCharacter.cpp", ["Characters/ArenaCharacter.h", "Components/HealthComponent.h",
                                                     "Weapons/ArenaWeapon.h", "ArenaStatics.h", "Subsystems/ScoreSubsystem.h",
                                                     "EnhancedInputComponent.h"])
    CH = Cls("AArenaCharacter", h, c)
    uclass(h, "UCLASS()", "AArenaCharacter", "ACharacter")
    h.add("public:", "\tAArenaCharacter();", "")
    CH.inline("UHealthComponent* GetHealth() const { return Health; }", "GetHealth")
    CH.decl("void EquipWeapon(AArenaWeapon* NewWeapon);", "EquipWeapon", "UFUNCTION(BlueprintCallable)")
    CH.decl("void StartFire();", "StartFire")
    h.add("protected:")
    CH.decl("virtual void BeginPlay() override;", "BeginPlay")
    CH.decl("virtual void SetupPlayerInputComponent(UInputComponent* PlayerInputComponent) override;", "SetupPlayerInputComponent")
    CH.decl("void HandleHealthChanged(float NewHealth, float Delta);", "HandleHealthChanged", "UFUNCTION()")
    h.add("\tUPROPERTY(VisibleAnywhere)", "\tTObjectPtr<UHealthComponent> Health;", "",
          "\tUPROPERTY(Replicated)", "\tTObjectPtr<AArenaWeapon> Weapon;", "",
          "\tUPROPERTY(EditDefaultsOnly)", "\tTSubclassOf<AArenaWeapon> DefaultWeaponClass;", "",
          "\tUPROPERTY(EditDefaultsOnly)", "\tTObjectPtr<UInputAction> FireAction;", "};")
    c.add("AArenaCharacter::AArenaCharacter()", "{", '\tHealth = CreateDefaultSubobject<UHealthComponent>(TEXT("Health"));', "}", "")
    CH.define("void AArenaCharacter::BeginPlay()", "BeginPlay", [
        "Super::BeginPlay();", "Health->OnHealthChanged.AddDynamic(this, &AArenaCharacter::HandleHealthChanged);",
        "if (DefaultWeaponClass)", "{", "\tAArenaWeapon* Spawned = GetWorld()->SpawnActor<AArenaWeapon>(DefaultWeaponClass);",
        "\tEquipWeapon(Spawned);", "}"])
    CH.define("void AArenaCharacter::SetupPlayerInputComponent(UInputComponent* PlayerInputComponent)", "SetupPlayerInputComponent", [
        "Super::SetupPlayerInputComponent(PlayerInputComponent);",
        "if (UEnhancedInputComponent* Input = Cast<UEnhancedInputComponent>(PlayerInputComponent))", "{",
        "\tInput->BindAction(FireAction, ETriggerEvent::Triggered, this, &AArenaCharacter::StartFire);", "}"])
    CH.define("void AArenaCharacter::StartFire()", "StartFire", ["if (Weapon)", "{", "\tWeapon->Fire();", "}"])
    CH.define("void AArenaCharacter::EquipWeapon(AArenaWeapon* NewWeapon)", "EquipWeapon", ["Weapon = NewWeapon;", "Weapon->SetOwner(this);"])
    CH.define("void AArenaCharacter::HandleHealthChanged(float NewHealth, float Delta)", "HandleHealthChanged", [
        "if (Health->IsDead())", "{", "\tUArenaStatics::GetScoreSubsystem(this)->AddKill(this);", "}"])
    T.call(CH.defs["BeginPlay"], CH.defs["HandleHealthChanged"], hard="callback-binding")
    T.call(CH.defs["BeginPlay"], CH.defs["EquipWeapon"])
    T.call(CH.defs["SetupPlayerInputComponent"], CH.defs["StartFire"], hard="callback-binding")
    T.call(CH.defs["StartFire"], W.defs["Fire"])
    T.call(CH.defs["HandleHealthChanged"], HC.defs["IsDead"])
    T.call(CH.defs["HandleHealthChanged"], ST.defs["GetScoreSubsystem"])
    T.call(CH.defs["HandleHealthChanged"], SS.defs["AddKill"], hard="chained-call")

    # ---- Weapon subclasses
    for name in WEAPONS[:n_weapons]:
        cls = f"A{name}"
        h = header(f"{PUB}/Weapons/{name}.h", ["Weapons/ArenaWeapon.h"])
        c = cpp(f"{PRI}/Weapons/{name}.cpp", [f"Weapons/{name}.h", "ArenaDamage.h", "ArenaStatics.h",
                                              "Characters/ArenaCharacter.h", "Components/HealthComponent.h", "Kismet/GameplayStatics.h"])
        X = Cls(cls, h, c)
        uclass(h, "UCLASS()", cls, "AArenaWeapon")
        h.add("protected:")
        X.decl("virtual void OnFire(const FVector& Aim) override;", "OnFire")
        X.decl("void MulticastImpact(const FVector& Where);", "MulticastImpact", "UFUNCTION(NetMulticast, Unreliable)")
        X.decl("AArenaCharacter* TraceTarget(const FVector& Aim) const;", "TraceTarget")
        h.add("\tUPROPERTY(EditDefaultsOnly)", "\tfloat Range = 5000.f;", "};")
        X.define(f"void {cls}::OnFire(const FVector& Aim)", "OnFire", [
            "Super::OnFire(Aim);", "FDamageInfo Info = ArenaDamage::MakeDamage(BaseDamage, Aim);",
            "Info.Amount *= ArenaDamage::Falloff(Range * 0.5f, Range);",
            "if (AArenaCharacter* Target = TraceTarget(Aim))", "{", "\tTarget->GetHealth()->ApplyDamage(Info);",
            "\tMulticastImpact(Target->GetActorLocation());", "}"])
        X.define(f"void {cls}::MulticastImpact_Implementation(const FVector& Where)", "MulticastImpact_Implementation",
                 ["UGameplayStatics::SpawnEmitterAtLocation(this, nullptr, Where);"])
        X.define(f"AArenaCharacter* {cls}::TraceTarget(const FVector& Aim) const", "TraceTarget", [
            "AActor* Hit = UArenaStatics::TraceForActor(this, Aim, Range);", "return Cast<AArenaCharacter>(Hit);"])
        o = X.defs["OnFire"]
        for t in (W.defs["OnFire"], MAKEDMG, FALLOFF, X.defs["TraceTarget"], CH.defs["GetHealth"]):
            T.call(o, t)
        T.call(o, HC.defs["ApplyDamage"], hard="chained-call")
        T.call(o, X.defs["MulticastImpact_Implementation"], hard="ue-rpc")
        T.call(X.defs["TraceTarget"], ST.defs["TraceForActor"])

    # ---- Enemies
    count = max(1, n_weapons * 4 // 5)
    h = header(f"{PUB}/Enemies/ArenaEnemy.h", ["Characters/ArenaCharacter.h"])
    c = cpp(f"{PRI}/Enemies/ArenaEnemy.cpp", ["Enemies/ArenaEnemy.h", "Kismet/GameplayStatics.h"])
    EN = Cls("AArenaEnemy", h, c)
    uclass(h, "UCLASS(Abstract)", "AArenaEnemy", "AArenaCharacter")
    h.add("public:", "\tAArenaEnemy();", "")
    EN.decl("virtual void Tick(float DeltaSeconds) override;", "Tick")
    h.add("protected:")
    EN.decl("AArenaCharacter* FindPlayer() const;", "FindPlayer")
    EN.decl("virtual void Attack(AArenaCharacter* Player);", "Attack")
    h.add("\tUPROPERTY(EditDefaultsOnly)", "\tfloat Strength = 5.f;", "};")
    c.add("AArenaEnemy::AArenaEnemy()", "{", "\tPrimaryActorTick.bCanEverTick = true;", "}", "")
    EN.define("void AArenaEnemy::Tick(float DeltaSeconds)", "Tick", [
        "Super::Tick(DeltaSeconds);", "if (AArenaCharacter* Player = FindPlayer())", "{", "\tAttack(Player);", "}"])
    EN.define("AArenaCharacter* AArenaEnemy::FindPlayer() const", "FindPlayer",
              ["return Cast<AArenaCharacter>(UGameplayStatics::GetPlayerCharacter(this, 0));"])
    EN.define("void AArenaEnemy::Attack(AArenaCharacter* Player)", "Attack", ["StartFire();"])
    T.call(EN.defs["Tick"], EN.defs["FindPlayer"])
    T.call(EN.defs["Tick"], EN.defs["Attack"])
    T.call(EN.defs["Attack"], CH.defs["StartFire"])
    for name in ENEMIES[:count]:
        cls = f"A{name}"
        h = header(f"{PUB}/Enemies/{name}.h", ["Enemies/ArenaEnemy.h"])
        c = cpp(f"{PRI}/Enemies/{name}.cpp", [f"Enemies/{name}.h", "ArenaDamage.h", "Components/HealthComponent.h"])
        X = Cls(cls, h, c)
        uclass(h, "UCLASS()", cls, "AArenaEnemy")
        h.add("protected:")
        X.decl("virtual void Attack(AArenaCharacter* Player) override;", "Attack")
        X.decl("void Taunt();", "Taunt")
        h.add("};")
        X.define(f"void {cls}::Attack(AArenaCharacter* Player)", "Attack", [
            "FDamageInfo Info = ArenaDamage::MakeDamage(Strength, GetActorForwardVector());",
            "Player->GetHealth()->ApplyDamage(Info);", "if (Player->GetHealth()->IsDead())", "{", "\tTaunt();", "}"])
        X.define(f"void {cls}::Taunt()", "Taunt", ["GetHealth()->Heal(Strength * 0.1f);"])
        a = X.defs["Attack"]
        for t in (MAKEDMG, CH.defs["GetHealth"], X.defs["Taunt"]):
            T.call(a, t)
        T.call(a, HC.defs["ApplyDamage"], hard="chained-call")
        T.call(a, HC.defs["IsDead"], hard="chained-call")
        T.call(X.defs["Taunt"], CH.defs["GetHealth"])
        T.call(X.defs["Taunt"], HC.defs["Heal"], hard="chained-call")

    # ---- Pickups
    h = header(f"{PUB}/Pickups/ArenaPickup.h", ["GameFramework/Actor.h"], ["AArenaCharacter"])
    c = cpp(f"{PRI}/Pickups/ArenaPickup.cpp", ["Pickups/ArenaPickup.h", "Characters/ArenaCharacter.h", "Components/HealthComponent.h"])
    PK = Cls("AArenaPickup", h, c)
    uclass(h, "UCLASS(Abstract)", "AArenaPickup", "AActor")
    h.add("public:")
    PK.decl("virtual void NotifyActorBeginOverlap(AActor* OtherActor) override;", "NotifyActorBeginOverlap")
    h.add("protected:")
    PK.decl("virtual void ApplyTo(AArenaCharacter* Character);", "ApplyTo")
    h.add("\tUPROPERTY(EditAnywhere)", "\tfloat Amount = 25.f;", "};")
    PK.define("void AArenaPickup::NotifyActorBeginOverlap(AActor* OtherActor)", "NotifyActorBeginOverlap", [
        "Super::NotifyActorBeginOverlap(OtherActor);", "if (AArenaCharacter* Character = Cast<AArenaCharacter>(OtherActor))", "{",
        "\tApplyTo(Character);", "\tDestroy();", "}"])
    PK.define("void AArenaPickup::ApplyTo(AArenaCharacter* Character)", "ApplyTo", ["Character->GetHealth()->Heal(Amount);"])
    T.call(PK.defs["NotifyActorBeginOverlap"], PK.defs["ApplyTo"])
    T.call(PK.defs["ApplyTo"], CH.defs["GetHealth"])
    T.call(PK.defs["ApplyTo"], HC.defs["Heal"], hard="chained-call")
    for name in PICKUPS[:count]:
        cls = f"A{name}"
        h = header(f"{PUB}/Pickups/{name}.h", ["Pickups/ArenaPickup.h"])
        c = cpp(f"{PRI}/Pickups/{name}.cpp", [f"Pickups/{name}.h", "ArenaStatics.h", "Subsystems/ScoreSubsystem.h"])
        X = Cls(cls, h, c)
        uclass(h, "UCLASS()", cls, "AArenaPickup")
        h.add("protected:")
        X.decl("virtual void ApplyTo(AArenaCharacter* Character) override;", "ApplyTo")
        h.add("\tUPROPERTY(EditAnywhere)", "\tint32 BonusPoints = 5;", "};")
        X.define(f"void {cls}::ApplyTo(AArenaCharacter* Character)", "ApplyTo", [
            "Super::ApplyTo(Character);", "if (UScoreSubsystem* Score = UArenaStatics::GetScoreSubsystem(this))", "{",
            "\tScore->AddBonus(BonusPoints);", "}"])
        for t in (PK.defs["ApplyTo"], ST.defs["GetScoreSubsystem"], SS.defs["AddBonus"]):
            T.call(X.defs["ApplyTo"], t)

    # ---- Game mode
    h = header(f"{PUB}/ArenaGameMode.h", ["GameFramework/GameModeBase.h"], ["AArenaEnemy"])
    c = cpp(f"{PRI}/ArenaGameMode.cpp", ["ArenaGameMode.h", "Enemies/ArenaEnemy.h", "Subsystems/ScoreSubsystem.h"])
    GM = Cls("AArenaGameMode", h, c)
    uclass(h, "UCLASS()", "AArenaGameMode", "AGameModeBase")
    h.add("public:")
    GM.decl("virtual void StartPlay() override;", "StartPlay")
    GM.decl("void SpawnWave(int32 Wave);", "SpawnWave", "UFUNCTION(BlueprintCallable)")
    h.add("protected:", "\tUPROPERTY(EditDefaultsOnly)", "\tTArray<TSubclassOf<AArenaEnemy>> EnemyClasses;", "};")
    GM.define("void AArenaGameMode::StartPlay()", "StartPlay", ["Super::StartPlay();", "SpawnWave(1);"])
    GM.define("void AArenaGameMode::SpawnWave(int32 Wave)", "SpawnWave", [
        "for (const TSubclassOf<AArenaEnemy>& EnemyClass : EnemyClasses)", "{",
        "\tGetWorld()->SpawnActor<AArenaEnemy>(EnemyClass, FVector::ZeroVector, FRotator::ZeroRotator);", "}",
        "if (UScoreSubsystem* Score = GetGameInstance()->GetSubsystem<UScoreSubsystem>())", "{", "\tScore->StartRound(Wave);", "}"])
    T.call(GM.defs["StartPlay"], GM.defs["SpawnWave"])
    T.call(GM.defs["SpawnWave"], SS.defs["StartRound"])

    for s_ in files:
        s_.write()
    T.dump(ROOT / "truth" / "arena.json")
    print(f"arena: {len(files)} files, {sum(len(s_.lines) for s_ in files)} lines, {len(T.defs)} defs, {len(T.calls)} calls")
