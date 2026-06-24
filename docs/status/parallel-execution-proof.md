# 验证记录:真并行步骤执行(scheduler fan-out)

Date: 2026-06-24
Status: PASS — 一个调度波次内的独立 ready 步骤,其工具子进程现在可并发执行(`--max-parallel N`),结果与串行**逐字节一致**;默认(0/1)与改造前**完全相同**。

## 实现(RFC Option A)

`run_step` 拆成三段(`crates/agentflow-core/src/runtime/mod.rs`):
- `prepare_step`:DB 读 + 暂存输入 + 物化 workdir + 写 runs/run_attempts 行 + 缓存命中/预运行校验失败的早退;构建好 `Command`。**主线程串行**。
- `run_local_command`:只跑子进程、不碰 DB。**可并发**。
- `record_step`:校验输出 + 注册产物/观测 + 写缓存 + finish_attempt。**主线程串行**。

`run_flow_with`:`max_parallel > 1` 时走 `run_ready_wave_parallel`——phase1 串行 prepare 整个波次,phase2 用 `std::thread::scope` 分批(每批 ≤ max_parallel)并发跑子进程,phase3 串行 record。**单个 SQLite 连接只在主线程使用,从不跨线程共享**,因此无数据竞争。`max_parallel ≤ 1` 时走原来的串行循环(逐字节不变)。

CLI:`--max-parallel N` 加到 `run` 与 `agent run`(`RunConfig.max_parallel`,默认 0 = 串行)。

## 证据

- **一致性测试** `parallel_execution_matches_serial_outputs_and_lineage`:同一个 fan-out 流(`narrow`/`wide`/`join`/`wide_tail`,`wide` 无依赖→与 `narrow` 同波次)串行 vs `max_parallel=4`,断言 `completed_steps=4`、`failed_steps=0`、每步 computed 输出 **byte-identical**。
- **真并行 live demo**:4 个独立 `sleep 1` 步骤(同波次)——
  - `--max-parallel 1`:`real 4.08s`(串行,4×1s)
  - `--max-parallel 4`:`real 1.02s`(并发,~1s);Completed 4 / Failed 0。
  约 **4× 加速**,证明子进程确实重叠。
- 默认路径不变:改造前的 **354 个测试全绿**(串行分支与原代码同一段);加并行后 core **355**(含新一致性测试)+ cli + acceptance 绿,clippy 干净,`argument.rs` 空 diff。

## 不变量

- "调度只改顺序、不改结果"扩展为"并行只改**何时何地**跑子进程、不改结果"——一致性测试守住。
- DB 串行(prepare/record 在主线程),无并发写;诚实/血缘不变量不受影响。

## 边界(诚实)

- 失败策略:波次按 ≤max_parallel 分批 prepare→execute→record;**某批出现失败即停止整个波次**(后续批次不 prepare 也不启动,不会遗留 running 行)——与串行"首个失败即停"对齐(同批内已启动的子进程仍跑完,这是并行的固有取舍)。一致性测试覆盖全成功场景。
- 同波次两步若 cache_key 相同(完全相同的 tool+inputs+params),会各自计算并写同一缓存键(幂等覆盖,同输出);独立步骤极少相同。
- 取消语义、容器后端的并行(多个 `docker run` 同时)受 `--max-parallel` 上界约束,未额外特殊处理。
- 典型线性链 fan-out 度常为 1,收益有限;价值在宽 fan-out(多独立分支)。
