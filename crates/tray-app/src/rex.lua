local list, err = rex.call("session.list")
if not list then
  error(err)
end
local blocks = {}
for _, session in ipairs(list.sessions or {}) do
  local id = session.session_id
  -- session.view answers only on a connection attached to that session.
  local view = (rex.call("session.attach", { session_id = id }) or {}).view or {}
  for _, window in ipairs(view.windows or {}) do
    for _, layer in ipairs(window.layers or {}) do
      for _, block in ipairs(layer.blocks or {}) do
        if block.creator_name == "com.superlogical.terminal" then
          -- rex.block.call fills in session_id only for the script's own session.
          local function call(method)
            return rex.call("block.method", { session_id = id, block_id = block.block_id, method = method })
          end
          local status = call("program_status")
          if status and type(status.records) == "table" and next(status.records) then
            table.insert(blocks, { block_id = block.block_id, program_status = status, process = call("process"), title = call("title") })
          end
        end
      end
    end
  end
end
return blocks
