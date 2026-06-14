return function(__lux_import)
  --#lux source: ..\examples\gmod_project\src\client\ui.lux:1
  local __lux_exports = {}
  local __lux_import_1 = __lux_import("lux/ui#client")
  local __lux_ui_node = __lux_import_1.node
  --#lux source: ..\examples\gmod_project\src\client\ui.lux:7
  local state
  --#lux source: ..\examples\gmod_project\src\client\ui.lux:12
  local mount
  --#lux source: ..\examples\gmod_project\src\client\ui.lux:24
  local view
  --#lux source: ..\examples\gmod_project\src\client\ui.lux:1
  do
  --#lux source: ..\examples\gmod_project\src\client\ui.lux:3
    local __lux_import_2 = __lux_import("gmod_project/hud#client")
    local buildHud = __lux_import_2.buildHud
  --#lux source: ..\examples\gmod_project\src\client\ui.lux:4
    local __lux_import_3 = __lux_import("lux/reactive#client")
    local signal = __lux_import_3.signal
  --#lux source: ..\examples\gmod_project\src\client\ui.lux:5
    local Button = __lux_import_1.Button
    local Column = __lux_import_1.Column
    local Label = __lux_import_1.Label
  --#lux source: ..\examples\gmod_project\src\client\ui.lux:7
    state = { count = signal(0), label = signal("Clicks") }
  --#lux source: ..\examples\gmod_project\src\client\ui.lux:12
    mount = function(panel, players, mode)
      if mode == nil then
        mode = "compact"
      end
      panel.Paint = function(self, w, h)
        local hud = buildHud(state.count(), players, mode)
        drawHud(self, w, h, hud.text)
      end
      panel.OnMousePressed = function()
        state.count(state.count() + 1)
        state.label(state.label() .. "!")
      end
    end
  --#lux source: ..\examples\gmod_project\src\client\ui.lux:24
    view = function(players, mode, ...)
      if mode == nil then
        mode = "detailed"
      end
      local hud = buildHud(state.count(), players, mode, ...)
      return __lux_ui_node(
        "Column",
        { gap = 8 },
        {
          __lux_ui_node("Label", { text = hud.text }, {}),
          __lux_ui_node(
            "Button",
            {
              text = tostring(state.label()) .. ": " .. tostring(state.count()),
              onClick = function()
                return state.count(state.count() + 1)
              end,
            },
            {}
          ),
        }
      )
    end
  --#lux source: ..\examples\gmod_project\src\client\ui.lux:1
  end
  
  --#lux source: ..\examples\gmod_project\src\client\ui.lux:12
  __lux_exports.mount = mount
  --#lux source: ..\examples\gmod_project\src\client\ui.lux:24
  __lux_exports.view = view
  
  --#lux source: ..\examples\gmod_project\src\client\ui.lux:1
  return __lux_exports
end
