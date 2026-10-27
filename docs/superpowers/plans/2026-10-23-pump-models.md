# Pump Models (LabQ, LabV, PDS II) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The feed pump and the refill pump can each be a LabQ, a LabV or a head of a PDS II, chosen in Settings > Pumps, and are driven exactly like the LabQ today.

**Architecture:** `fermentool-modbus` gains a `PumpKind` (model + PDS II head) that yields a `RegisterMap`; a `Pump` addresses a `PumpTarget` (address, kind) instead of a bare address, so a wrong map cannot be written. `config.toml` carries model / head per pump; max rpm and rpm step are constants of the model, in the code; the engine bounds every rpm by the model's max and snaps it to the model's step, switches LabV / PDS II to transferring mode before driving, and refuses a tubing calibration made on another pump model. The UI follows mockup v3.

**Tech Stack:** Rust (workspace crates `fermentool-modbus`, `fermentool-core`, axum, rusqlite), Svelte 5 + Vite, vitest, headless-Edge smoke test.

**Spec:** `docs/superpowers/specs/2026-10-23-pump-models-design.md` (read it first). Mockup: https://claude.ai/artifact/JhLH9s8ue5mjEUBiJsspGf (v3).

## Global Constraints

- Never build a frame for a head or tubing register (LabQ/LabV 1000, 1001; PDS II n000, n001) nor for PDS II register 0010 (all start/stop), in daemon code.
- Register maps, decimal: LabQ speed 1002, flow 1004, start/stop 1006, direction 1007, no mode. LabV speed 1002, flow 1004, start/stop 1008, direction 1009, mode 1020. PDS II head n (1..=8): speed n·1000+32, flow n·1000+30, start/stop n·1000+3, direction n·1000+4, mode n·1000+5. Transferring mode value = 0 for both.
- Max rpm fixed per model, in the code, no setting (decided 2026-10-26): LabQ 350, LabV 150 (the bench's LabV is a LabV1-II, 0.1..150 rpm), PDS II 600. Floor 0.1.
- Setpoint step per model: rpm LabQ 0.1 (today's grid, unchanged), LabV 0.01 (checked on a LabV1-II on 2026-10-26: 68.39 rpm typed is kept), PDS II 0.01 (checked on the bench's PDS II on 2026-10-26, rpm and ml/min); ml/min LabQ 0.001 (today's grid, unchanged), LabV 0.01 (its manual: flow rate resolution 0.01 ml/min), PDS II 0.01. The floor stays 0.1 rpm for every model: the step is the grid, not the minimum.
- Addresses: LabQ 1..=247, LabV and PDS II 1..=32.
- Config strings: `"labq"`, `"labv"`, `"pds2"`. A config without them is LabQ + LabQ, max 350.
- Cable shown as "9600 baud · 8E1"; `serial.baud` stays in `config.toml`.
- UI strings in English; never "burst" in UI text; no em dash and no spaced hyphen as one anywhere (code, comments, commits).
- Commit messages: no `Co-Authored-By` trailer, no `Claude-Session` line (project rule).
- Build from the VS Community environment: `cmd /c '"C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat" && cargo ...'`. During an active run, heavy work at low priority (`start "" /b /low /wait ...`). Retry once on `STATUS_HEAP_CORRUPTION`.
- After any change under `ui/src/`: from `ui/`, `npm run check` (0 errors, 0 warnings), `npm test`, `npm run build`.

## Review Focus

1. An existing install (no model in `config.toml`) after the update: the LabQ frames must be byte-for-byte what they are today. Test in Task 1 (`labq_frames_unchanged`).
2. Feed and refill on the same address: refused for LabQ / LabV, and for two PDS II on the same head; allowed for two PDS II heads. Tests in Task 3 (`same_pump_rule`) and Task 7 (`pumpClash`).
3. The refill pump changed to a model whose max is under the refill speed already saved (a LabQ at 300 rpm made a LabV): the save is refused with a message naming both figures, never a silent clamp. Tests in Task 3 (`refill_speed_over_its_max_is_refused`) and Task 7.
4. A crash-resume on a LabV / PDS II already in transferring mode: no mode write, no Stop, the feed is not interrupted. Test in Task 4 (`resume_in_transferring_mode_writes_no_mode`).
5. The feed pump's model changed after a calibration was chosen in New run (a stale page, or the API): the daemon refuses it with a hint. Test in Task 6 (`calibration_of_another_model_is_refused`).

---

### Task 1: Pump kinds and register maps (fermentool-modbus)

**Files:**
- Modify: `crates/fermentool-modbus/src/lib.rs` (module doc, `reg`, `limits`, `PumpTransport`, `Pump`, `SimPump` storage)
- Test: same file, `mod tests`

**Interfaces:**
- Produces:
  - `pub enum PumpModel { LabQ, LabV, PdsII }` with `pub fn as_str(self) -> &'static str` (`"labq"`, `"labv"`, `"pds2"`), `pub fn parse(s: &str) -> Option<PumpModel>`, `pub fn max_rpm(self) -> f64` (350 / 150 / 600), `pub fn max_address(self) -> u8` (247 / 32 / 32), `pub fn rpm_step(self) -> f64` (0.1 / 0.01 / 0.01), `pub fn flow_step(self) -> f64` (0.001 / 0.01 / 0.01), `pub fn label(self) -> &'static str` ("LabQ", "LabV", "PDS II").
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum PumpKind { LabQ, LabV, PdsII { head: u8 } }` with `pub fn new(model: PumpModel, head: u8) -> Result<PumpKind, PumpError>` (head 1..=8 checked for PDS II, ignored otherwise), `pub fn model(self) -> PumpModel`, `pub fn map(self) -> RegisterMap`, `pub fn head(self) -> Option<u8>`.
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub struct RegisterMap { pub speed: u16, pub flow: u16, pub start_stop: u16, pub direction: u16, pub mode: Option<u16> }`, `pub const TRANSFERRING: u16 = 0;`.
  - `#[derive(Debug, Clone, Copy, PartialEq)] pub struct PumpTarget { pub address: u8, pub kind: PumpKind }` with `pub fn labq(address: u8) -> PumpTarget`.
  - `Pump::new(transport, address)` unchanged (LabQ target, max 350); `Pump::with_target(transport, PumpTarget) -> Pump<T>`; `pump.target() -> PumpTarget`; `pump.set_target(PumpTarget)`; `pump.with_target_of<R>(target: PumpTarget, f: impl FnOnce(&mut Self) -> R) -> R` (the refill pump; restores the own target). `address()`, `set_address()` and `with_address()` keep working (same kind and max, other address).
  - `PumpTransport::ensure_transferring(&mut self) -> Result<bool, PumpError>`: `Ok(false)` without a mode register or when it reads 0; otherwise Stop, write 0, `Ok(true)`.
  - `set_speed_rpm` refuses outside `0.1..=target.kind.model().max_rpm()` with `PumpError::OutOfRange` (message built from the figure: make `OutOfRange` carry a `String`, update its users).
  - `limits::RPM_MAX` is removed; keep `RPM_MIN`, `FLOW_MIN`, `FLOW_MAX`. `reg::*` LabQ constants stay (tests and the simulator use them) with a note that the daemon goes through `RegisterMap`.
  - `SimPump` stores every holding register (`BTreeMap<u16, u16>`, any address accepted); existing LabQ accessors (`speed_rpm()`, `running()`, ...) unchanged; new `pub fn reg(&self, reg: u16) -> u16`, `pub fn set_reg(&mut self, reg: u16, value: u16)`, `pub fn speed_rpm_of(&self, map: RegisterMap) -> f32`, `pub fn running_of(&self, map: RegisterMap) -> bool`. Faults (`reject_setpoint_writes`, `frozen_readback`) apply to any map's speed / flow pair (`is_setpoint_reg`: register % 1000 in {2, 4, 30, 32}).

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn register_maps_follow_the_protocols() {
    assert_eq!(PumpKind::LabQ.map(), RegisterMap { speed: 1002, flow: 1004, start_stop: 1006, direction: 1007, mode: None });
    assert_eq!(PumpKind::LabV.map(), RegisterMap { speed: 1002, flow: 1004, start_stop: 1008, direction: 1009, mode: Some(1020) });
    assert_eq!(PumpKind::PdsII { head: 3 }.map(), RegisterMap { speed: 3032, flow: 3030, start_stop: 3003, direction: 3004, mode: Some(3005) });
    assert!(PumpKind::new(PumpModel::PdsII, 0).is_err());
    assert!(PumpKind::new(PumpModel::PdsII, 9).is_err());
}

#[test]
fn labq_frames_unchanged() {
    // byte-for-byte what Pump::new built before this change
    let mut p = Pump::new(SimPump::new(1), 1);
    p.start().unwrap();
    p.set_speed_rpm(58.8).unwrap();
    assert!(p.transport().running());
    assert!((p.transport().speed_rpm() - 58.8).abs() < 1e-4);
    assert_eq!(frame::build_write_single(1, PumpKind::LabQ.map().start_stop, 1), frame::build_write_single(1, reg::START_STOP, 1));
}

#[test]
fn a_labv_target_writes_the_labv_registers() { /* Pump::with_target(SimPump::new(4), PumpTarget { address: 4, kind: PumpKind::LabV }); start(); transport().reg(1008) == 1 and reg(1006) == 0 */ }

#[test]
fn speed_over_the_target_max_is_refused() { /* LabV max 150: set_speed_rpm(150.1) is Err(OutOfRange), 150.0 is Ok */ }

#[test]
fn ensure_transferring_reads_first() {
    // LabQ: Ok(false), no transaction. PDS II head 2 with mode register 2005 at 1:
    // Ok(true), 2003 == 0 (stopped) and 2005 == 0. Called again: Ok(false), no write.
}

#[test]
fn no_frame_for_head_tubing_or_all_start() {
    // every register in every map of every kind (heads 1..=8) is none of
    // 1000, 1001, 10, n000, n001
}
```

- [ ] **Step 2: Run, expect failures (types not defined)**

Run: `cargo test -p fermentool-modbus --no-default-features`
Expected: compile errors naming `PumpKind`, `RegisterMap`.

- [ ] **Step 3: Implement the types, the target in `Pump`, `ensure_transferring`, the per-target range check; replace `limits::RPM_MAX` users inside the crate**

- [ ] **Step 4: Run the crate's tests**

Run: `cargo test -p fermentool-modbus --no-default-features`
Expected: PASS. (`fermentool-core` does not compile yet where it used `limits::RPM_MAX`: Task 4 fixes it. Do not commit a broken workspace: in this task, add a temporary `pub const RPM_MAX: f64 = 350.0;` marked `#[deprecated(note = "use the pump model's max_rpm()")]`, removed in Task 4.)

- [ ] **Step 5: `cargo test` for the workspace passes, commit**

```bash
git add crates/fermentool-modbus/src/lib.rs
git commit -m "feat(modbus): LabV and PDS II register maps behind a pump target"
```

### Task 2: A simulator for every model (fermentool-modbus)

**Files:**
- Modify: `crates/fermentool-modbus/src/lib.rs` (`SimPump`, `SimBus`)
- Test: same file

**Interfaces:**
- Consumes: Task 1 (`PumpKind`, `RegisterMap`).
- Consumes: Task 1's register storage (`reg`, `set_reg`).
- Produces:
  - `pub strict: Option<PumpModel>`: when set, the SimPump answers like that model: exception 02 for a register that model does not document (LabQ 1000..=1009; LabV 1000..=1005, 1007..=1010, 1015..=1016, 1018..=1023; PDS II 10 and, for n 1..=8, n000..=n035 and n100..=n107); exception 03 for a speed / flow write while that map's mode register is not 0; no answer (timeout) for a PDS II head listed in `pub logical_heads: BTreeSet<u8>`.
  - `SimBus::pump(address)` unchanged (non-strict pumps, as the daemon's `"sim"` path uses).

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn a_strict_labv_refuses_the_labq_start_register() {
    let mut sim = SimPump::new(1);
    sim.strict = Some(PumpModel::LabV);
    let mut p = Pump::new(sim, 1); // the LabQ map on a LabV: the anti-goal
    assert!(matches!(p.start(), Err(PumpError::Pdu(PduError::Exception(0x02)))));
    assert_eq!(p.transport().reg(1008), 0);
}

#[test]
fn a_strict_pds_refuses_speed_outside_transferring() { /* head 1, set_reg(1005, 1): set_speed_rpm -> Exception(0x03); ensure_transferring then set_speed_rpm -> Ok */ }

#[test]
fn a_pds_head_in_logical_operation_is_silent() { /* logical_heads {2}: read_speed_rpm on head 2 -> Err(Transport(Timeout)); head 1 answers */ }
```

- [ ] **Step 2: Run, expect failures**

Run: `cargo test -p fermentool-modbus --no-default-features`

- [ ] **Step 3: Implement the `strict` behaviours**

- [ ] **Step 4: Run, expect PASS; `cargo test` for the workspace passes**

- [ ] **Step 5: Commit**

```bash
git commit -am "test(modbus): simulate the LabV and the PDS II heads"
```

### Task 3: Config per pump: model, head

**Files:**
- Modify: `crates/fermentool-core/src/config.rs` (`PumpConfig`, `RefillConfig`, `validate`, `sanitize`, defaults, `REFILL_RPM_MAX`)
- Test: same file

**Interfaces:**
- Consumes: Task 1 (`PumpModel`, `PumpKind`, `PumpTarget`).
- Produces:
  - `#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)] #[serde(rename_all = "lowercase")] pub enum PumpModelCfg { #[default] Labq, Labv, Pds2 }` with `pub fn model(self) -> PumpModel` (the modbus crate stays without serde).
  - `PumpConfig { address: u8, model: PumpModelCfg, head: u8 /* default 1 */ }`, `pub fn target(&self) -> PumpTarget`. No `max_rpm` key: a `max_rpm` left in an edited `config.toml` is ignored (serde default, unknown keys tolerated as today).
  - `RefillConfig` gains `pump_model: PumpModelCfg`, `pump_head: u8`, same `target()`.
  - `pub fn same_pump(a: &PumpTarget, b: &PumpTarget) -> bool`: same address and (not both PDS II, or same head).
  - `REFILL_RPM_MAX` removed: the refill speed is bounded by the refill pump model's `max_rpm()`.
  - `validate()` refuses: address over the model's `max_address()`; head outside 1..=8 for a PDS II; refill `speed_rpm` over the refill pump model's `max_rpm()` ("the refill pump's speed 300 rpm is over its max 150 rpm"). `sanitize()` resets each of these to its default with a note, as today. The feed/refill clash stays a run-time check (Task 5), as today, so a config with both at 2 still saves.

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn an_old_config_is_labq_and_labq() { /* Config::from_toml("[pump]\naddress = 1\n") -> model Labq, head 1, target().kind.model().max_rpm() 350.0; refill same */ }

#[test]
fn max_rpm_is_the_model_s() { /* PumpModel::LabQ.max_rpm() 350, LabV 150, PdsII 600; rpm_step 0.1 / 0.01 / 0.01, flow_step 0.001 / 0.01 / 0.01; a config.toml with max_rpm = 400 under [pump] loads and is ignored */ }

#[test]
fn addresses_follow_the_model() { /* labq 200 Ok; labv 33 Err; pds2 head 9 Err */ }

#[test]
fn same_pump_rule() {
    // labq 2 vs labq 2: true; labv 2 vs labq 2: true; pds2 3 head 1 vs pds2 3 head 2: false; pds2 3 head 1 vs pds2 3 head 1: true
}

#[test]
fn refill_speed_over_its_max_is_refused() { /* refill labv (max 150), speed_rpm 300 -> validate Err containing "300" and "150"; sanitize resets [refill] with a note */ }
```

- [ ] **Step 2: Run, expect failures**

Run: `cargo test -p fermentool-core config::`

- [ ] **Step 3: Implement; update `api.rs` tests that set `refill.pump_address` if a message changed**

- [ ] **Step 4: Run, expect PASS; workspace `cargo test` passes**

- [ ] **Step 5: Commit**

```bash
git commit -am "feat(config): model and head for the feed and refill pumps"
```

### Task 4: The engine drives the feed pump by its target

**Files:**
- Modify: `crates/fermentool-core/src/engine/mod.rs` (fields `pump_addr`, `set_serial`, `swap_transport`, `start_run`, `resume`, `drive_rpm`, `drive_write`, `EngineError`, `EngineStatus`), `crates/fermentool-core/src/control.rs` (`Command::Reconnect`), `crates/fermentool-core/src/transport.rs` (`SwapTransport` keeps `pump_addr: u8`), `crates/fermentool-core/src/main.rs:161`, `crates/fermentool-core/src/api.rs` (`ReconnectReq`, `serial_reconnect`, `serial_disconnect`, `create_run`)
- Test: `engine/mod.rs` tests, `control.rs` tests, `api.rs` tests

**Interfaces:**
- Consumes: Task 1 `PumpTarget`, `ensure_transferring`; Task 3 `PumpConfig::target()`.
- Produces:
  - `Engine::set_serial(&mut self, serial: SerialConfig, feed: PumpTarget)`; `Engine::swap_transport(&mut self, serial: &SerialConfig, feed: PumpTarget)`; `Command::Reconnect { serial, feed: PumpTarget, reply }`. The engine's `pump_addr: u8` field becomes `feed: PumpTarget` (`self.feed.address` where the address was used).
  - `setpoint_grid` and `drive_grid` take the rpm step: `fn setpoint_grid(control_var: ControlVar, model: PumpModel) -> f64` (`Rpm` → `model.rpm_step()`, `MlMin` → `model.flow_step()`), `fn drive_grid(control_var: ControlVar, drive: Option<f64>, model: PumpModel) -> f64`, every caller passes `self.feed.kind.model()`. A LabQ keeps today's grids exactly (0.1 rpm, 0.001 ml/min).
  - `fn drive_rpm(ml_min: f64, ml_per_rpm: f64, max_rpm: f64, rpm_step: f64) -> f64`; `start_run` clamps rpm curves to `(RPM_MIN, self.feed.kind.model().max_rpm())` and refuses a driven curve over it, message "this tube delivers at most X ml/min at N rpm" with N the pump's max.
  - `start_run` and `resume` call `self.pump.ensure_transferring()` before the first write: start before the start sequence; resume only on the path that writes to the pump (a pump answering 0 sees no write). A failure is `EngineError::PumpMode(PumpError)` with `hint()`: LabV "Put the LabV on its main screen; Fermentool sets transferring mode itself, it could not.", PDS II "Is head n set to independent operation, and the controller on its main screen?".
  - `EngineStatus` gains `pump_model: &'static str` (`"labq"`...), `pump_head: Option<u8>`, `pump_max_rpm: f64`, `pump_rpm_step: f64`, `pump_flow_step: f64` (from the model, read only, for the UI).
  - `ReconnectReq` gains `pump_model: Option<String>`, `pump_head: Option<u8>`. `serial_reconnect` sets them into `cfg.pump`, runs `cfg.validate()` (400 on refusal) before the swap. A run active: refused by the engine as today (409).
  - `create_run` keeps `cfg.pump_addr = ...address` (journal compatibility).
  - Remove the temporary deprecated `limits::RPM_MAX` from Task 1.

- [ ] **Step 1: Failing tests (engine)**

```rust
#[test]
fn a_labv_feed_run_writes_the_labv_registers() { /* Engine on Pump::with_target(strict LabV SimPump, LabV target max 150); start an rpm run at 100 rpm: reg 1008 == 1, speed_rpm_of(LabV map) == 100 */ }

#[test]
fn a_curve_over_the_pump_max_is_clamped_or_refused() { /* LabV max 150: rpm curve end 200 -> clamp_max 150 in the stored curve; ml/min driven in rpm with peak / r > 150 -> Err(Config) naming 150 */ }

#[test]
fn a_pds_in_dispensing_mode_is_switched_once() { /* strict PdsII head 2, set_reg(2005, 1): start_run Ok, reg 2005 == 0, reg 2003 == 1 */ }

#[test]
fn resume_in_transferring_mode_writes_no_mode() { /* strict PdsII head 1 running, 1005 == 0: crash then resume; transactions between resume start and first setpoint include no write to 1005 and no write of 0 to 1003 */ }

#[test]
fn a_silent_pds_head_refuses_the_start_with_a_hint() { /* logical_heads {1}: start_run Err(PumpMode(..)), detail().hint mentions "independent operation" */ }

#[test]
fn the_rpm_grid_follows_the_model() { /* LabV feed: an rpm run whose curve asks 68.394 writes 68.39 (speed_rpm_of(LabV map)); an ml/min run driven in rpm moves by 0.01 rpm steps; an ml/min run on a PDS II writes 12.35 for 12.346; the same on a LabQ writes 68.4 and 12.346, as today */ }

#[test]
fn status_reports_the_feed_model() { /* pump_model "pds2", pump_head Some(2), pump_max_rpm 600.0, pump_rpm_step 0.01, pump_flow_step 0.01 */ }
```

API test: `POST /api/serial/reconnect` with `{"pump_model":"labv","pump_head":1}` saves `model = "labv"` in `config.toml` and `/api/status` says `"pump_model":"labv"`; with `"pump_model":"labv"` and an address 40 answers 400.

- [ ] **Step 2: Run, expect failures**

Run: `cargo test -p fermentool-core engine:: control:: api::`

- [ ] **Step 3: Implement; update every caller and test helper of `set_serial` / `swap_transport` / `Reconnect` (`PumpTarget::labq(addr)` where they passed an address)**

- [ ] **Step 4: Full `cargo test` passes**

- [ ] **Step 5: Commit**

```bash
git commit -am "feat(engine): drive the feed pump by its model and head"
```

### Task 5: The refill pump by its target

**Files:**
- Modify: `crates/fermentool-core/src/api.rs` (`put_config`), `crates/fermentool-core/src/control.rs` (`Command::SetRefill` reply type), `crates/fermentool-core/src/engine/refill.rs` (`set_refill`, `refill_test`, `refill_ping`, `refill_status`, `RefillStatus`), `crates/fermentool-core/src/engine/refill_cycle.rs` (`refill_start_pump`, `refill_speed_now`, every `with_address(cfg.pump_address, ..)`)
- Test: both files' tests

**Interfaces:**
- Consumes: Task 3 `RefillConfig::target()`, `same_pump`; Task 4 `self.feed`.
- Produces:
  - Every refill frame goes through `self.pump.with_target_of(self.refill.cfg.target(), ..)`.
  - The clash checks in `refill_test`, `refill_ping` and `refill_before_run` use `same_pump(&refill, &self.feed)`; messages: LabQ / LabV unchanged ("Address N is the feed pump's ..."), PDS II "Head n of address N is the feed pump's: pick another head."
  - `set_refill` treats a changed model or head like a moved address (`moved`).
  - `refill_start_pump` and `refill_test` call `ensure_transferring()` before speed / direction / start.
  - `RefillStatus` gains `model: &'static str`, `head: Option<u8>`.
  - `Engine::set_refill` returns `Result<(), String>` and refuses a changed `pump_model` or `pump_head` while a run drives the refill pump ("Not during a run with automatic refill: the refill pump's model and head are locked until it stops."); `put_config` answers it with 409 before saving (as `SetScale` is handled), so `config.toml` never runs ahead of the engine.

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn feed_and_refill_on_two_heads_of_one_pds() { /* SimBus with strict PdsII at 3; feed head 1, refill head 2: refill_ping Ok(true); refill_test writes 2003 = 1, never 1003 */ }

#[test]
fn the_same_pds_head_is_refused() { /* refill head 1 = feed head 1: refill_ping Err containing "Head 1" */ }

#[test]
fn the_refill_model_is_locked_during_an_auto_refill_run() { /* auto-refill run active: set_refill with pump_model Labv -> Err; same model, other speed -> Ok */ }

#[test]
fn a_fill_switches_a_labv_refill_pump_to_transferring() { /* strict LabV at 2, 1020 = 1: a fill starts, 1020 == 0, 1008 == 1 */ }
```

- [ ] **Step 2: Run, expect failures**

Run: `cargo test -p fermentool-core refill`

- [ ] **Step 3: Implement**

- [ ] **Step 4: Full `cargo test` passes**

- [ ] **Step 5: Commit**

```bash
git commit -am "feat(refill): the refill pump by its model and head"
```

### Task 6: Calibrations belong to a pump model; runs record theirs

**Files:**
- Create: `crates/fermentool-core/src/store/migrations/0014_pump_model.sql`
- Modify: `crates/fermentool-core/src/store/mod.rs` (migration list, `NewCalibration`, `CalibrationRow`, `CAL_COLS`, `insert_calibration`, run insert / `RunRow`), `crates/fermentool-core/src/api.rs` (`create_calibration`, `create_run`), `crates/fermentool-core/src/engine/mod.rs` (`start_run` calibration check, `EngineError`)
- Test: `store/mod.rs`, `engine/mod.rs`, `api.rs` tests

**Interfaces:**
- Consumes: Task 3 `PumpModelCfg`; Task 4 `self.feed`.
- Produces:
  - Migration:
    ```sql
    -- The pump model a tube was calibrated on: a calibration (ml per rpm)
    -- is only true on that model's head. Every calibration before this was
    -- measured on the LabQ.
    ALTER TABLE tubing_calibrations ADD COLUMN pump_model TEXT NOT NULL DEFAULT 'labq';
    -- The feed pump a run drove, for its History detail.
    ALTER TABLE runs ADD COLUMN pump_model TEXT NOT NULL DEFAULT 'labq';
    ALTER TABLE runs ADD COLUMN pump_head INTEGER;
    ```
  - `NewCalibration.pump_model: String` set by `create_calibration` from the config's feed model (never from the request body; a body value is ignored). `CalibrationRow.pump_model: String`, in the JSON of `/api/calibrations`.
  - The run row gets `pump_model` / `pump_head` from `self.feed` in `start_run`; `RunRow` exposes them (`/api/runs/{id}` JSON).
  - `EngineError::CalibrationOtherPump { cal_model: String, pump_model: String }`, `code` `"calibration_other_pump"`, Display "this tube was calibrated on a LabQ, the feed pump is a LabV", `hint()` "A calibration in ml per rpm is only true on the pump head it was measured on. Calibrate this tube on the LabV (Tubing calibration), or set the feed pump back in Settings > Pumps." Checked in `start_run` with the other calibration checks. `resume` does not check it (a crash resume never refuses on a calibration, as today).

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn old_calibrations_are_labq() { /* a db migrated from 0013 with one calibration: calibration(id).pump_model == "labq" */ }

#[test]
fn calibration_of_another_model_is_refused() { /* calibration with pump_model "labq", feed target LabV: start_run Err(CalibrationOtherPump); feed LabQ: Ok */ }

#[test]
fn a_run_records_its_feed_pump() { /* feed PdsII head 2: run(id).pump_model == "pds2", pump_head == Some(2) */ }
```

API test: `POST /api/calibrations` with config feed model `labv` stores `"pump_model":"labv"` even when the body says `"labq"`.

- [ ] **Step 2: Run, expect failures**

Run: `cargo test -p fermentool-core store:: engine:: api::`

- [ ] **Step 3: Implement**

- [ ] **Step 4: Full `cargo test` passes**

- [ ] **Step 5: Commit**

```bash
git commit -am "feat(calibration): a tube calibration belongs to the pump model it was measured on"
```

### Task 7: UI logic for the pump models (`ui/src/lib`)

**Files:**
- Modify: `ui/src/lib/pumps.js`, `ui/src/lib/refill.js`, `ui/src/lib/fmt.js` (`RPM_LIMITS`), `ui/src/lib/tuberange.js`
- Create: `ui/src/lib/pumps.test.js` if absent (else extend), extend `ui/src/lib/refill.test.js`, `tuberange` test
- Create: `ui/src/lib/calibrations.js` + `calibrations.test.js`

**Interfaces:**
- Consumes: the status / config fields of Tasks 4 to 6 (`pump_model`, `pump_head`, `pump_max_rpm`, `pump_rpm_step`, `pump_flow_step`, `refill.pump_model`...).
- Produces (all plain functions):
  - `MODELS = [{ id: 'labq', label: 'LabQ', maxRpm: 350, maxAddress: 247, rpmStep: 0.1, flowStep: 0.001 }, { id: 'labv', label: 'LabV', maxRpm: 150, maxAddress: 32, rpmStep: 0.01, flowStep: 0.01 }, { id: 'pds2', label: 'PDS II', maxRpm: 600, maxAddress: 32, rpmStep: 0.01, flowStep: 0.01 }]`, `modelOf(id)`. Constants, mirroring `PumpModel` (no setting).
  - `samePump(a, b)` with `{ address, model, head }`, the Task 3 rule.
  - `pumpProblem({ model, address, head })` → message or null.
  - `refillProblem({ address, model, head, speedRpm, feed })` (max from `modelOf(model).maxRpm`) replaces today's signature (feed = `{ address, model, head }`), messages as Task 5 and "The speed must be between 0.1 and {maxRpm} rpm (the refill pump's max)."
  - `notAnsweringHint({ model, address, head })`: LabQ / LabV "Is the {label} switched on, at address {address} and 9600 baud in its own menu?"; PDS II "Is the PDS II switched on, at address {address}, 9600 baud, and head {head} set to independent operation?".
  - `needsReconnect(saved, now)` also true when the feed model or head changed.
  - `RPM_LIMITS` stays the floor only; `tubeRange(...)` takes `maxRpm` (from `app.status.pump_max_rpm`, 350 when absent) and its messages say it.
  - `calibrations.js`: `forFeedPump(cals, pumpModel)` keeps rows whose `pump_model` equals it (a row without it counts as `'labq'`).

- [ ] **Step 1: Failing vitest cases**

```js
test('pumpClash: two heads of one PDS II share an address', () => {
  expect(samePump({ address: 3, model: 'pds2', head: 1 }, { address: 3, model: 'pds2', head: 2 })).toBe(false);
  expect(samePump({ address: 3, model: 'pds2', head: 1 }, { address: 3, model: 'pds2', head: 1 })).toBe(true);
  expect(samePump({ address: 2, model: 'labv' }, { address: 2, model: 'labq' })).toBe(true);
});
test('refill speed over its max is refused with both figures', () => {
  expect(refillProblem({ address: 2, model: 'labv', head: 1, speedRpm: 300, feed: { address: 1, model: 'labq' } })).toMatch(/150/);
});
test('the PDS II hint names the head', () => {
  expect(notAnsweringHint({ model: 'pds2', address: 3, head: 2 })).toMatch(/head 2 set to independent operation/);
});
test('New run offers only the feed model calibrations', () => {
  expect(forFeedPump([{ id: 1 }, { id: 2, pump_model: 'labv' }], 'labq').map((c) => c.id)).toEqual([1]);
});
```

- [ ] **Step 2: `npm test` from `ui/`, expect failures**

- [ ] **Step 3: Implement**

- [ ] **Step 4: `npm test` and `npm run check` pass**

- [ ] **Step 5: Commit**

```bash
git commit -am "feat(ui): pump model rules, hints and limits as tested functions"
```

### Task 8: Settings > Pumps, as mockup v3

**Files:**
- Create: `ui/src/assets/pumps/labq.jpg`, `labv.jpg`, `pds2.jpg` (from the mockup's photos: LabQ 3/4 view, LabV1 with AMC 12-channel head, PDS II controller with its heads; trimmed on white, about 300 px, JPEG q 86, a few tens of kB each; the session's scratchpad `photos/` has the sources, else re-fetch from the URLs in the spec's mockup notes)
- Modify: `ui/src/routes/settings/PumpsSettings.svelte`, `ui/tests/smoke.mjs`

**Interfaces:**
- Consumes: Task 7 functions; Task 4 reconnect body fields; Task 3 config fields.

- [ ] **Step 1: Smoke checks first (they fail on today's page)**

In the Pumps block of `ui/tests/smoke.mjs`, looking only, changing nothing:
- each `.pumps .item` has an `img` in `.pic` and a model radiogroup with LabQ, LabV, PDS II;
- the cable line shows `9600 baud` and no baud `select`;
- clicking PDS II on the refill row shows a `Head` select and a "Head n" badge, then the page is left without Save (navigate away, re-open, the saved model is back).

- [ ] **Step 2: Implement the rows as the mockup**

Photo tile 120 × 96, white (`--surface` is not white in dark mode: add a `--photo-bg: #ffffff` token to all three theme blocks of `app.css`), border `--line`, a problem row draws its border `--danger`. Model segmented control with `radioKeys` / `rovingTab` (as the direction control), Address (`max` from the model), Head select only for PDS II, no max rpm field (it is the model's). The refill speed field's `max` and `step` are the refill pump model's `maxRpm` and `rpmStep` (today `step="1"`). The cable line: port select, rescan, and a `.bus` text "9600 baud · 8E1". The not-answering line uses `notAnsweringHint`. Save: `needsReconnect` → `POST /api/serial/reconnect` with `pump_model`, `pump_head`; refill fields through `patchConfig` (`pump_model`, `pump_head`). Model, head and address of the feed pump disabled during a run (as the address today); the refill pump's model and head disabled while `app.status.refill?.in_run`.

- [ ] **Step 3: `npm run check`, `npm test`, `npm run build` pass**

- [ ] **Step 4: Look at it on a local copy** (`vite preview` on the build, `#settings/pumps`, light and dark, a simulated status with `pump_model: 'pds2'`), never on the real pump.

- [ ] **Step 5: Commit (with `ui/dist`)**

```bash
git commit -am "feat(settings): choose each pump's model and head, with its photo"
```

### Task 9: New run, Tubing calibration, History

**Files:**
- Modify: `ui/src/routes/NewRun.svelte` (`calMatches`, the tube range), `ui/src/routes/TubingCalibration.svelte` (the model on each calibration row), `ui/src/routes/History.svelte` / `ui/src/lib/rundetail.js` (Settings fold), `ui/tests/smoke.mjs`

**Interfaces:**
- Consumes: Task 7 `forFeedPump`, `tubeRange(..., maxRpm)`; Task 6 JSON fields.

- [ ] **Step 1: Smoke checks** (looking only): New run's tube list holds only calibrations of the status `pump_model`; a calibration row on the Tubing calibration page shows its model label; a run detail's Settings fold shows "Feed pump: LabQ" (or "PDS II, head 2").

- [ ] **Step 2: Implement.** `calMatches` = `forFeedPump(...)` then today's filter. When calibrations exist but none for this model, the tube list says "No tube calibrated on the {label} yet: calibrate one in Tubing calibration." The run detail line comes from a tested `rundetail.js` function `pumpLine(run)` ("LabQ", "LabV", "PDS II, head 2"), add its vitest case. In New run, the curve's Start / End / Value fields take `step` from `modelOf(app.status.pump_model)`: `rpmStep` for an rpm curve (0.01 on a LabV or PDS II, so 68.39 is valid; 0.1 on a LabQ, as today), `flowStep` for an ml/min curve.

- [ ] **Step 3: `npm run check`, `npm test`, `npm run build` pass**

- [ ] **Step 4: Commit (with `ui/dist`)**

```bash
git commit -am "feat(ui): calibrations and runs show their pump model"
```

### Task 10: Docs, deploy, smoke

**Files:**
- Create: `docs/LabV Series MODBUS protocol.md` (from the PDF, `pdftotext -layout`), `docs/PDS Series MODBUS protocol.md` (the user's copy)
- Modify: `docs/wiring.md` (LabV DB9: pin 7 GND1 RS-485 signal ground, 8 B, 9 A; PDS II: to be filled from the pump), `CLAUDE.md` (Conventions: the head/tubing rule for all three models, the 0010 ban, the model's max rpm (LabQ 350, LabV 150, PDS II 600) and rpm step (0.1 / 0.01 / 0.01) and ml/min step (0.001 / 0.01 / 0.01) replace "Refused if the curve needs more than 350 rpm"; the `limits::RPM_MAX` mentions), `CHANGELOG.md`, `crates/fermentool-modbus/Cargo.toml` description

- [ ] **Step 1: Write the docs**

- [ ] **Step 2: Full verification**: `cargo test`; from `ui/`: `npm run check`, `npm test`, `npm run build`.

- [ ] **Step 3: Deploy per CLAUDE.md** (re-check `/api/status` for an active run right before; no run active: `POST /api/shutdown`, `cargo build --release`, start `target\release\fermentool-core.exe` with `FERMENTOOL_NO_BROWSER=1`, `/api/status` shows `"pump_model":"labq"`, the served bundle contains "8E1"). Then `npm run smoke`. Tell the user to Ctrl+F5.

- [ ] **Step 4: Commit**

```bash
git commit -am "docs: LabV and PDS II protocols, wiring, rules for every pump model"
```

- [ ] **Step 5: Hand the hardware checklist to the user** (spec, Tests, "Hardware"): on a real LabV and a PDS II, Settings > Pumps answers, a short rpm run starts / stops / turns the chosen way, a pump left in another mode is switched, a speed over the max is refused, the LabV's parity is even.
