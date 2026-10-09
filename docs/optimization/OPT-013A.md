# OPT-013A：AI 流式完成信号与错误分型

Status: verified-local
父卡OPT-013的独立子卡；完整长文恢复/版本缓存/设置继承由父卡后续完成，不因本卡关闭父卡。前置：OPT002安全已CI。

## 目标

stream_chat仍返回AppResult<ChatOutcome>兼容调用者；只有模型明确finish_reason=stop并正常结束SSE才completed=true。length→aiTruncated Err，content_filter→aiFiltered Err，tool_calls/未知/EOF无finish→aiIncomplete或protocol Err，不把HTTP EOF当完成。消费方sink false仍completed=false取消且不缓存。

兼容性：官方Chat Completion正常finish_reason stop可跟[DONE]或干净EOF（两者确定策略在Note，优先要求stop且[DONE]或正常关闭）；只有[DONE]无reason不能称自然完整。choices空的usage帧不误报；error:null忽略，error object有明确错误；非JSON data、坏UTF8不能静默丢弃并假完成。重复/冲突finish_reason及DONE之后数据拒绝或按清晰终态处理，不能越界拼接。

保留流式delta用户已看到的文本，由命令层Err路径使前端显示失败和可重试，不需要新增UI接口；不完整产物不得落完成缓存。commands/ai保存DB失败必须Err而非Done假成功。后台只读无真实AI调用。

## 写集

- src-tauri/src/ai.rs（解析状态机和内部单测）
- src-tauri/src/commands/ai.rs（不完整/DB失败正常传播；不改IPC事件形状或prompt配置）
- src-tauri/tests/ai_stream_completion_e2e.rs（新增严格本地SSE夹具）
- src-tauri/tests/ai_e2e.rs（现有mock若没有finish_reason需按真实完整帧补齐，保留断言）
- .agents/notes/implemented/architecture/2026-10-08-AI完成信号与缓存提交.md（新Note）
- tmp/optimization-20261008/OPT-013A/RESULT.md

不得改db/schema/前端/store/lib注册（其他cards工作）；不要增加对模型token预算/功能删减。

## TDD具体反例

- delta+finish stop+DONE →完整文本；跨TCP chunk UTF8、CRLF、尾帧无换行照样正确。
- delta+length+DONE →Err aiTruncated；delta+content_filter→Err；只有delta后干净EOF、只有DONE无reason→Err。
- error:null与usage空choices正常；error object、JSON乱码、坏UTF8不是成功；sink false→completedfalse。
- output上限仍1024/4096；不通过调大数字掩盖错误。SSE行缓冲与累计text都有有限预算，达到上限明确失败。
- 完整命令缓存提交失败不发done/假成功（可模块测试DBtrigger注入），不能只测试ChatOutcome存在。

## 完成

先写真实stream_chat HTTP红测再实现状态机，cargo tests ai相关与clippy lib，Note和RESULT真实记录；不运行真实付费接口；主控独立review与验收。

## R3 验收

Parfit独立R3 PASS；主控ai_e2e1项与ai_stream_completion_e2e29项退出0。BOM/事件行/CRLF边界/预算/DONE及时完成/取消与缓存写失败闭环，父卡长文恢复/版本缓存未关闭。
