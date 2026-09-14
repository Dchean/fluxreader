<!-- project-workflow: generated view; edit task JSON instead -->
# 参考方案调研

状态：not_needed

试点为纯内部移动：把项目自己的 db.rs 按既有领域函数分组拆成 db/ 子模块，不引入外部库或算法。项目已具备针对性的设计材料（docs/runs/M2-module-disposition-20260913.md、.agents/notes/proposed/architecture/2026-09-13-m2-module-disposition.md），其中已给出拆分方案与“不引入 ORM”的备选比较；实现只遵循项目现有 Rust 模块约定，无外部实现可借鉴。
