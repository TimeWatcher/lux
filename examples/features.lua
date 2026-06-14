local __lux_exports = {}
local arrayHelpers
local callbackExamples
local classify
local defaultAndDestructure
local demo
local doExpressionDemo
local fillPreviewColor
local gmodStdDemo
local helper
local isPointerAction
local macroExpressionExample
local macroStatementExample
local macroValue
local mergeTableProps
local multivalueDemo
local mutateCounter
local namespaceImport
local noImplicitReturn
local passthrough
local pipelineHelpers
local reactiveDemo
local requirePanelName
local safeDotCall
local safeLookup
local sumUntilNegative
local summarize
local tableAndTailCalls
local themePadding
local tierForExp
local tierLabel

local __lux_import_1 = __lux_import("lux/ui")
local __lux_ui_node = __lux_import_1.node
local __lux_import_2 = __lux_import("@lux/std")
local arr = __lux_import_2.arr
local std = __lux_import_2
local __lux_import_3 = __lux_import("@lux/reactive")
local memo = __lux_import_3.memo
local signal = __lux_import_3.signal
local __lux_import_4 = __lux_import("@lux/gmod")
local color = __lux_import_4.color
local hookx = __lux_import_4.hookx
local timerx = __lux_import_4.timerx
local valid = __lux_import_4.valid
local vgui = __lux_import_4.vgui
local featureVersion = "0.1"
local defaultTitle = "visitor"
--#lux source: ..\examples\features.lux:36
passthrough = function(...)
  return ...
end
--#lux source: ..\examples\features.lux:38
helper = function(x)
  return x + 1
end
--#lux source: ..\examples\features.lux:40
macroValue = function(x)
  do
    local __lux_macro_dbg_1 = x + 1
    print("..\\examples\\features.lux:41", __lux_macro_dbg_1)
    return __lux_macro_dbg_1
  end
end
--#lux source: ..\examples\features.lux:43
tierForExp = function(exp)
--#lux source: ..\examples\features.lux:44
  if exp == nil then
    return 0
  end
--#lux source: ..\examples\features.lux:45
  if exp < 5 then
    return 0
  end
--#lux source: ..\examples\features.lux:46
  if exp < 25 then
    return 1
  end
  return 2
end
--#lux source: ..\examples\features.lux:50
tierLabel = function(tier)
  local __lux_match_5 = tier
--#lux source: ..\examples\features.lux:52
  if __lux_match_5 == 0 then
    return "guest"
--#lux source: ..\examples\features.lux:53
  elseif __lux_match_5 == 1 then
    return "regular"
--#lux source: ..\examples\features.lux:54
  elseif __lux_match_5 == 2 then
    return "veteran"
  end
end
--#lux source: ..\examples\features.lux:57
classify = function(player)
  local __lux_obj_6 = player
  local __lux_val_8 = nil
--#lux source: ..\examples\features.lux:58
  if __lux_obj_6 ~= nil then
    local __lux_method_7 = __lux_obj_6.GetExp
    if __lux_method_7 ~= nil then
      __lux_val_8 = __lux_method_7(__lux_obj_6)
    end
  end
  local __lux_tmp_9 = __lux_val_8
  if __lux_tmp_9 == nil then
    __lux_tmp_9 = 0
  end
  return tierLabel(tierForExp(__lux_tmp_9))
end
--#lux source: ..\examples\features.lux:60
themePadding = function(theme)
  local __lux_match_10 = theme
--#lux source: ..\examples\features.lux:62
  if __lux_match_10 == "compact" then
    return 4
--#lux source: ..\examples\features.lux:63
  elseif __lux_match_10 == "spacious" then
    return 12
  end
end
--#lux source: ..\examples\features.lux:66
isPointerAction = function(action)
  local __lux_match_11 = action
--#lux source: ..\examples\features.lux:68
  if __lux_match_11 == "hover" or __lux_match_11 == "press" or __lux_match_11 == "release" then
    return true
--#lux source: ..\examples\features.lux:69
  elseif __lux_match_11 == "cancel" then
    return false
  end
end
--#lux source: ..\examples\features.lux:72
fillPreviewColor = function(fill)
  local __lux_match_12 = fill
  local __lux_tag_13 = __lux_match_12.kind
--#lux source: ..\examples\features.lux:74
  if __lux_tag_13 == FILL_SOLID then
    local color = __lux_match_12.color
    return color
--#lux source: ..\examples\features.lux:75
  elseif __lux_tag_13 == FILL_LINEAR then
    local from = __lux_match_12.from
    local to = __lux_match_12.to
    return color.lerp(0.5, from, to)
  end
end
--#lux source: ..\examples\features.lux:78
sumUntilNegative = function(values)
  local total = 0
--#lux source: ..\examples\features.lux:81
  for i = 1, #values do
    local __lux_break_14 = false
    repeat
      local value = values[i]
--#lux source: ..\examples\features.lux:83
      if value == nil then
        break
      end
--#lux source: ..\examples\features.lux:84
      if value < 0 then
        __lux_break_14 = true
        break
      end
      total = total + value
--#lux source: ..\examples\features.lux:81
    until true
    if __lux_break_14 then
      break
    end
  end
  return total
end
--#lux source: ..\examples\features.lux:91
requirePanelName = function(panel)
--#lux source: ..\examples\features.lux:92
  if not valid.is(panel) then
    return "invalid-panel"
  end
  return panel:GetName()
end
--#lux source: ..\examples\features.lux:96
summarize = function(player, fallbackName)
  local name
  do
    local __lux_obj_15 = player
    local __lux_val_17 = nil
--#lux source: ..\examples\features.lux:97
    if __lux_obj_15 ~= nil then
      local __lux_method_16 = __lux_obj_15.GetName
      if __lux_method_16 ~= nil then
        __lux_val_17 = __lux_method_16(__lux_obj_15)
      end
    end
    local __lux_tmp_18 = __lux_val_17
    if __lux_tmp_18 == nil then
      __lux_tmp_18 = fallbackName
    end
    name = __lux_tmp_18
  end
  local exp
  do
    local __lux_obj_19 = player
    local __lux_val_21 = nil
--#lux source: ..\examples\features.lux:98
    if __lux_obj_19 ~= nil then
      local __lux_method_20 = __lux_obj_19.GetExp
      if __lux_method_20 ~= nil then
        __lux_val_21 = __lux_method_20(__lux_obj_19)
      end
    end
    local __lux_tmp_22 = __lux_val_21
    if __lux_tmp_22 == nil then
      __lux_tmp_22 = 0
    end
    exp = __lux_tmp_22
  end
  local title
--#lux source: ..\examples\features.lux:99
  if exp > 10 then
    title = "veteran"
  else
    title = defaultTitle
  end
  return "Player " .. tostring(name) .. ": " .. tostring(title)
end
--#lux source: ..\examples\features.lux:104
mutateCounter = function(state)
  state.count = state.count + 1
  state.label = state.label .. "!"
  return state.count
end
--#lux source: ..\examples\features.lux:110
noImplicitReturn = function()
  mutateCounter({ count = 0, label = "dry-run" })
end
--#lux source: ..\examples\features.lux:115
safeLookup = function(tbl, keyFactory)
  local __lux_obj_23 = tbl
  local __lux_val_25 = nil
--#lux source: ..\examples\features.lux:116
  if __lux_obj_23 ~= nil then
    local __lux_key_24 = keyFactory()
    __lux_val_25 = __lux_obj_23[__lux_key_24]
  end
  local __lux_tmp_26 = __lux_val_25
  if __lux_tmp_26 == nil then
    __lux_tmp_26 = "missing"
  end
  return __lux_tmp_26
end
--#lux source: ..\examples\features.lux:118
safeDotCall = function(mgfx, x, y)
  local __lux_obj_27 = mgfx
  local __lux_val_29 = nil
--#lux source: ..\examples\features.lux:119
  if __lux_obj_27 ~= nil then
    local __lux_fn_28 = __lux_obj_27.RoundedBox
    if __lux_fn_28 ~= nil then
      __lux_val_29 = __lux_fn_28(expensiveRadius(), x, y)
    end
  end
  return __lux_val_29
end
--#lux source: ..\examples\features.lux:121
defaultAndDestructure = function(player, fallbackArmor)
  if fallbackArmor == nil then
    fallbackArmor = 0
  end
  local name, hp, armor
  do
    local __lux_destructure_30 = player
    local __lux_field_31 = __lux_destructure_30.name
    name = __lux_field_31
    local __lux_field_32 = __lux_destructure_30.hp
    hp = __lux_field_32
    local __lux_field_33 = __lux_destructure_30.armor
--#lux source: ..\examples\features.lux:122
    if __lux_field_33 == nil then
      __lux_field_33 = fallbackArmor
    end
    armor = __lux_field_33
  end
  local x, y, z
  do
    local __lux_tmp_34 = player.position
--#lux source: ..\examples\features.lux:123
    if __lux_tmp_34 == nil then
      __lux_tmp_34 = { 0, 0 }
    end
    local __lux_destructure_35 = __lux_tmp_34
    local __lux_item_36 = __lux_destructure_35[1]
    x = __lux_item_36
    local __lux_item_37 = __lux_destructure_35[2]
    y = __lux_item_37
    local __lux_item_38 = __lux_destructure_35[3]
    if __lux_item_38 == nil then
      __lux_item_38 = 0
    end
    z = __lux_item_38
  end
  return { name = name, hp = hp, armor = armor, x = x, y = y, z = z }
end
--#lux source: ..\examples\features.lux:135
mergeTableProps = function(base, label)
  local __lux_table_39 = {}
  local __lux_spread_40 = base
--#lux source: ..\examples\features.lux:136
  if __lux_spread_40 ~= nil then
    for __lux_k_41, __lux_v_42 in pairs(__lux_spread_40) do
      __lux_table_39[__lux_k_41] = __lux_v_42
    end
  end
  __lux_table_39.text = label
  __lux_table_39.visible = true
  return __lux_table_39
end
--#lux source: ..\examples\features.lux:138
pipelineHelpers = function(xs)
  local __lux_pipe_43 = xs
  local __lux_pipe_44 = arr.filter(
    __lux_pipe_43,
    function(x)
      return x ~= nil
    end
  )
  return arr.map(
    __lux_pipe_44,
    function(x, index)
      return x + index
    end
  )
end
--#lux source: ..\examples\features.lux:143
doExpressionDemo = function(state)
  local nextCount
  do
    local current
    do
      local __lux_tmp_45 = state.count
--#lux source: ..\examples\features.lux:145
      if __lux_tmp_45 == nil then
        __lux_tmp_45 = 0
      end
      current = __lux_tmp_45
    end
    nextCount = current + 1
  end
  return nextCount
end
--#lux source: ..\examples\features.lux:152
callbackExamples = function(panel)
--#lux source: ..\examples\features.lux:153
  panel.Paint = function(self, w, h)
    local color
    if self.isActive then
      color = "green"
    else
      color = "gray"
    end
    drawPanel(self, w, h, color)
  end
--#lux source: ..\examples\features.lux:158
  panel.OnClick = function()
    local __lux_tmp_46 = panel.count
    if __lux_tmp_46 == nil then
      __lux_tmp_46 = 0
    end
    return helper(__lux_tmp_46)
  end
end
--#lux source: ..\examples\features.lux:161
tableAndTailCalls = function(count)
  __lux_ui_node("Label", { text = "Count: " .. tostring(count) }, {})
  local __lux_tmp_47
--#lux source: ..\examples\features.lux:169
  if count > 10 then
    __lux_tmp_47 = "Big"
  else
    __lux_tmp_47 = "Add"
  end
  return __lux_ui_node(
    "Column",
    { gap = 8 },
    {
      __lux_ui_node("Label", { text = "child" }, {}),
      __lux_ui_node(
        "Button",
        {
          text = __lux_tmp_47,
--#lux source: ..\examples\features.lux:166
          onClick = function()
            return count + 1
          end,
        },
        {}
      ),
    }
  )
end
--#lux source: ..\examples\features.lux:175
arrayHelpers = function(xs)
  return arr.map(
    xs,
    function(x, index)
      return x + index
    end
  )
end
--#lux source: ..\examples\features.lux:178
namespaceImport = function(xs)
  return std.arr.filter(
    xs,
    function(x)
      return x ~= nil
    end
  )
end
--#lux source: ..\examples\features.lux:181
reactiveDemo = function(seed)
  local count = signal(seed)
  local doubled = memo(
    function()
      return count() * 2
    end
  )
  count(count() + 1)
  return { count = count(), doubled = doubled() }
end
--#lux source: ..\examples\features.lux:193
gmodStdDemo = function(panel)
  local hookHandle = hookx.add(
    "Think",
    "LuxExampleThink",
    function()
      return nil
    end
  )
  local timerHandle = timerx.after(
    1,
    function()
      return print("later")
    end,
    "LuxExampleTimer"
  )
  local mixed = color.lerp(0.5, Color(255, 64, 32, 255), Color(32, 128, 255, 128))
  return {
    panelValid = valid.is(panel),
    removedChildren = vgui.clearChildren(panel),
    hookId = hookHandle.id,
    timerId = timerHandle.id,
    colorHex = color.toHex(mixed),
  }
end
--#lux source: ..\examples\features.lux:211
multivalueDemo = function(...)
  local a, b = passthrough(...)
  local c, d = passthrough(...), passthrough(...)
  local packed = { "head", passthrough(...) }
  return passthrough(...), passthrough(...)
end
--#lux source: ..\examples\features.lux:218
macroStatementExample = function()
  do
    local __lux_macro_hook_event_2 = "HUDPaint"
    local __lux_macro_hook_id_3 = "LuxExampleHUD"
--#lux source: ..\examples\features.lux:220
    local __lux_macro_hook_callback_4 = function()
      return drawLuxHud()
    end
    hook.Add(__lux_macro_hook_event_2, __lux_macro_hook_id_3, __lux_macro_hook_callback_4)
  end
end
--#lux source: ..\examples\features.lux:223
macroExpressionExample = function()
  do
    local __lux_macro_hook_event_5 = "HUDPaint"
--#lux source: ..\examples\features.lux:224
    local __lux_macro_hook_callback_7 = function()
      return drawLuxHud()
    end
    local __lux_macro_hook_id_6 = "__lux:hook:8"
    hook.Add(__lux_macro_hook_event_5, __lux_macro_hook_id_6, __lux_macro_hook_callback_7)
    return __lux_macro_hook_id_6
  end
end
--#lux source: ..\examples\features.lux:226
demo = function(player, panel, state, xs, keyFactory, ...)
  local summary = summarize(player, "Unknown")
  local kind = classify(player)
  local changed = mutateCounter(state)
  local suppressed = noImplicitReturn()
  local item = safeLookup(xs, keyFactory)
  local tier
  do
    local __lux_obj_48 = player
    local __lux_val_50 = nil
--#lux source: ..\examples\features.lux:232
    if __lux_obj_48 ~= nil then
      local __lux_method_49 = __lux_obj_48.GetExp
      if __lux_method_49 ~= nil then
        __lux_val_50 = __lux_method_49(__lux_obj_48)
      end
    end
    local __lux_tmp_51 = __lux_val_50
    if __lux_tmp_51 == nil then
      __lux_tmp_51 = 0
    end
    tier = tierForExp(__lux_tmp_51)
  end
  local tierText = tierLabel(tier)
  local padding = themePadding("spacious")
  local pointerAction = isPointerAction("press")
  local previewColor = fillPreviewColor(
    { kind = FILL_LINEAR, from = Color(255, 64, 32, 255), to = Color(32, 128, 255, 255) }
  )
  local summed = sumUntilNegative(xs)
  local panelName = requirePanelName(panel)
  local mapped = arrayHelpers(xs)
  local filtered = namespaceImport(xs)
  local reactive = reactiveDemo(state.count)
  local macroed = macroValue(state.count)
  local destructured = defaultAndDestructure(player)
  local mergedProps = mergeTableProps({ align = "left" }, summary)
  local piped = pipelineHelpers(xs)
  local nextCount = doExpressionDemo(state)
  local gmodStd = gmodStdDemo(panel)
  local removableHookId = macroExpressionExample()
  local first, second = multivalueDemo(...)
  return {
    summary = summary,
    kind = kind,
    changed = changed,
    suppressed = suppressed,
    item = item,
    tier = tier,
    tierText = tierText,
    padding = padding,
    pointerAction = pointerAction,
    previewColor = previewColor,
    summed = summed,
    panelName = panelName,
    mapped = mapped,
    filtered = filtered,
    reactive = reactive,
    macroed = macroed,
    destructured = destructured,
    mergedProps = mergedProps,
    piped = piped,
    nextCount = nextCount,
    gmodStd = gmodStd,
    removableHookId = removableHookId,
    first = first,
    second = second,
  }
end
--#lux source: ..\examples\features.lux:283
function PANEL:Paint(w, h)
  drawBody(self, w, h)
end

__lux_exports.featureVersion = featureVersion
__lux_exports.demo = demo

return __lux_exports
