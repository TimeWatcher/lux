local __lux_exports = {}
local arrayHelpers
local callbackExamples
local classify
local debugValue
local defaultAndDestructure
local demo
local doExpressionDemo
local fillPreviewColor
local filterArray
local gmodGlobalsDemo
local helper
local isPointerAction
local lerpChannel
local lerpColor
local mapArray
local mergeTableProps
local multivalueDemo
local mutateCounter
local noImplicitReturn
local passthrough
local pipelineHelpers
local requirePanelName
local safeDotCall
local safeLookup
local sumUntilNegative
local summarize
local tailCallDemo
local themePadding
local tierForExp
local tierLabel
local traceTail

local featureVersion = "0.2"
local defaultTitle = "visitor"
--#lux source: examples\features.lux:27
passthrough = function(...)
  return ...
end
--#lux source: examples\features.lux:29
helper = function(x)
  return x + 1
end
--#lux source: examples\features.lux:31
debugValue = function(x)
  local value = x + 1
  print("feature debug", value)
  return value
end
--#lux source: examples\features.lux:37
mapArray = function(values, project)
  local out = {}
--#lux source: examples\features.lux:40
  for i = 1, #values do
    out[#out + 1] = project(values[i], i)
  end
  return out
end
--#lux source: examples\features.lux:47
filterArray = function(values, predicate)
  local out = {}
--#lux source: examples\features.lux:50
  for i = 1, #values do
    repeat
      local value = values[i]
--#lux source: examples\features.lux:52
      if value == nil then
        break
      end
--#lux source: examples\features.lux:53
      if not predicate(value, i) then
        break
      end
      out[#out + 1] = value
--#lux source: examples\features.lux:50
    until true
  end
  return out
end
--#lux source: examples\features.lux:60
traceTail = function(value)
  return value
end
--#lux source: examples\features.lux:62
tailCallDemo = function(count)
  return traceTail({ text = "Count: " .. tostring(count), visible = true })
end
--#lux source: examples\features.lux:65
lerpChannel = function(t, from, to)
  return from + (to - from) * t
end
--#lux source: examples\features.lux:67
lerpColor = function(t, from, to)
  return Color(
    math.floor(lerpChannel(t, from.r, to.r)),
    math.floor(lerpChannel(t, from.g, to.g)),
    math.floor(lerpChannel(t, from.b, to.b)),
    math.floor(lerpChannel(t, from.a, to.a))
  )
end
--#lux source: examples\features.lux:75
fillPreviewColor = function(fill)
  local __lux_match_1 = fill
  local __lux_tag_2 = __lux_match_1.kind
--#lux source: examples\features.lux:77
  if __lux_tag_2 == FILL_SOLID then
    local color = __lux_match_1.color
    return color
--#lux source: examples\features.lux:78
  elseif __lux_tag_2 == FILL_LINEAR then
    local from = __lux_match_1.from
    local to = __lux_match_1.to
    return lerpColor(0.5, from, to)
  end
end
--#lux source: examples\features.lux:81
tierForExp = function(exp)
--#lux source: examples\features.lux:82
  if exp == nil then
    return 0
  end
--#lux source: examples\features.lux:83
  if exp < 5 then
    return 0
  end
--#lux source: examples\features.lux:84
  if exp < 25 then
    return 1
  end
  return 2
end
--#lux source: examples\features.lux:88
tierLabel = function(tier)
  local __lux_match_3 = tier
--#lux source: examples\features.lux:90
  if __lux_match_3 == 0 then
    return "guest"
--#lux source: examples\features.lux:91
  elseif __lux_match_3 == 1 then
    return "regular"
--#lux source: examples\features.lux:92
  elseif __lux_match_3 == 2 then
    return "veteran"
  end
end
--#lux source: examples\features.lux:95
classify = function(player)
  local __lux_obj_player_4 = player
  local __lux_val_GetExp_6 = nil
--#lux source: examples\features.lux:96
  if __lux_obj_player_4 ~= nil then
    local __lux_method_GetExp_5 = __lux_obj_player_4.GetExp
    if __lux_method_GetExp_5 ~= nil then
      __lux_val_GetExp_6 = __lux_method_GetExp_5(__lux_obj_player_4)
    end
  end
  local __lux_tmp_GetExp_7 = __lux_val_GetExp_6
  if __lux_tmp_GetExp_7 == nil then
    __lux_tmp_GetExp_7 = 0
  end
  return tierLabel(tierForExp(__lux_tmp_GetExp_7))
end
--#lux source: examples\features.lux:98
themePadding = function(theme)
  local __lux_match_8 = theme
--#lux source: examples\features.lux:100
  if __lux_match_8 == "compact" then
    return 4
--#lux source: examples\features.lux:101
  elseif __lux_match_8 == "spacious" then
    return 12
  end
end
--#lux source: examples\features.lux:104
isPointerAction = function(action)
  local __lux_match_9 = action
--#lux source: examples\features.lux:106
  if __lux_match_9 == "hover" or __lux_match_9 == "press" or __lux_match_9 == "release" then
    return true
--#lux source: examples\features.lux:107
  elseif __lux_match_9 == "cancel" then
    return false
  end
end
--#lux source: examples\features.lux:110
sumUntilNegative = function(values)
  local total = 0
--#lux source: examples\features.lux:113
  for i = 1, #values do
    local __lux_break_10 = false
    repeat
      local value = values[i]
--#lux source: examples\features.lux:115
      if value == nil then
        break
      end
--#lux source: examples\features.lux:116
      if value < 0 then
        __lux_break_10 = true
        break
      end
      total = total + value
--#lux source: examples\features.lux:113
    until true
    if __lux_break_10 then
      break
    end
  end
  return total
end
--#lux source: examples\features.lux:123
requirePanelName = function(panel)
--#lux source: examples\features.lux:124
  if not IsValid(panel) then
    return "invalid-panel"
  end
  local __lux_obj_panel_11 = panel
  local __lux_val_GetName_13 = nil
--#lux source: examples\features.lux:125
  if __lux_obj_panel_11 ~= nil then
    local __lux_method_GetName_12 = __lux_obj_panel_11.GetName
    if __lux_method_GetName_12 ~= nil then
      __lux_val_GetName_13 = __lux_method_GetName_12(__lux_obj_panel_11)
    end
  end
  return __lux_val_GetName_13
end
--#lux source: examples\features.lux:128
summarize = function(player, fallbackName)
  local name
  do
    local __lux_obj_player_14 = player
    local __lux_val_GetName_16 = nil
--#lux source: examples\features.lux:129
    if __lux_obj_player_14 ~= nil then
      local __lux_method_GetName_15 = __lux_obj_player_14.GetName
      if __lux_method_GetName_15 ~= nil then
        __lux_val_GetName_16 = __lux_method_GetName_15(__lux_obj_player_14)
      end
    end
    name = __lux_val_GetName_16
    if name == nil then
      name = fallbackName
    end
  end
  local exp
  do
    local __lux_obj_player_17 = player
    local __lux_val_GetExp_19 = nil
--#lux source: examples\features.lux:130
    if __lux_obj_player_17 ~= nil then
      local __lux_method_GetExp_18 = __lux_obj_player_17.GetExp
      if __lux_method_GetExp_18 ~= nil then
        __lux_val_GetExp_19 = __lux_method_GetExp_18(__lux_obj_player_17)
      end
    end
    exp = __lux_val_GetExp_19
    if exp == nil then
      exp = 0
    end
  end
  local title
--#lux source: examples\features.lux:131
  if exp > 10 then
    title = "veteran"
  else
    title = defaultTitle
  end
  return "Player " .. tostring(name) .. ": " .. tostring(title)
end
--#lux source: examples\features.lux:136
mutateCounter = function(state)
  state.count = state.count + 1
  state.label = state.label .. "!"
  return state.count
end
--#lux source: examples\features.lux:142
noImplicitReturn = function()
  mutateCounter({ count = 0, label = "dry-run" })
end
--#lux source: examples\features.lux:147
safeLookup = function(tbl, keyFactory)
  local __lux_obj_tbl_20 = tbl
  local __lux_val_tbl_22 = nil
--#lux source: examples\features.lux:148
  if __lux_obj_tbl_20 ~= nil then
    local __lux_key_keyFactory_21 = keyFactory()
    __lux_val_tbl_22 = __lux_obj_tbl_20[__lux_key_keyFactory_21]
  end
  local __lux_tmp_keyFactory_23 = __lux_val_tbl_22
  if __lux_tmp_keyFactory_23 == nil then
    __lux_tmp_keyFactory_23 = "missing"
  end
  return __lux_tmp_keyFactory_23
end
--#lux source: examples\features.lux:150
safeDotCall = function(mgfx, x, y)
  local __lux_obj_mgfx_24 = mgfx
  local __lux_val_RoundedBox_26 = nil
--#lux source: examples\features.lux:151
  if __lux_obj_mgfx_24 ~= nil then
    local __lux_fn_RoundedBox_25 = __lux_obj_mgfx_24.RoundedBox
    if __lux_fn_RoundedBox_25 ~= nil then
      __lux_val_RoundedBox_26 = __lux_fn_RoundedBox_25(expensiveRadius(), x, y)
    end
  end
  return __lux_val_RoundedBox_26
end
--#lux source: examples\features.lux:153
defaultAndDestructure = function(player, fallbackArmor)
  if fallbackArmor == nil then
    fallbackArmor = 0
  end
  local name, hp, armor
  do
    local __lux_destructure_27 = player
    local __lux_field_28 = __lux_destructure_27.name
    name = __lux_field_28
    local __lux_field_29 = __lux_destructure_27.hp
    hp = __lux_field_29
    local __lux_field_30 = __lux_destructure_27.armor
--#lux source: examples\features.lux:154
    if __lux_field_30 == nil then
      __lux_field_30 = fallbackArmor
    end
    armor = __lux_field_30
  end
  local x, y, z
  do
    local __lux_tmp_position_31 = player.position
--#lux source: examples\features.lux:155
    if __lux_tmp_position_31 == nil then
      __lux_tmp_position_31 = { 0, 0 }
    end
    local __lux_destructure_32 = __lux_tmp_position_31
    local __lux_item_33 = __lux_destructure_32[1]
    x = __lux_item_33
    local __lux_item_34 = __lux_destructure_32[2]
    y = __lux_item_34
    local __lux_item_35 = __lux_destructure_32[3]
    if __lux_item_35 == nil then
      __lux_item_35 = 0
    end
    z = __lux_item_35
  end
  return { name = name, hp = hp, armor = armor, x = x, y = y, z = z }
end
--#lux source: examples\features.lux:167
mergeTableProps = function(base, label)
  local __lux_table_36 = {}
  local __lux_spread_37 = base
--#lux source: examples\features.lux:168
  if __lux_spread_37 ~= nil then
    for __lux_k_38, __lux_v_39 in pairs(__lux_spread_37) do
      __lux_table_36[__lux_k_38] = __lux_v_39
    end
  end
  __lux_table_36.text = label
  __lux_table_36.visible = true
  return __lux_table_36
end
--#lux source: examples\features.lux:170
pipelineHelpers = function(xs)
  local __lux_pipe_40 = xs
  local __lux_pipe_41 = filterArray(
    __lux_pipe_40,
    function(x)
      return x ~= nil
    end
  )
  return mapArray(
    __lux_pipe_41,
    function(x, index)
      return x + index
    end
  )
end
--#lux source: examples\features.lux:175
arrayHelpers = function(xs)
  return mapArray(
    xs,
    function(x, index)
      return x + index
    end
  )
end
--#lux source: examples\features.lux:178
doExpressionDemo = function(state)
  local nextCount
  do
    local current = state.count
--#lux source: examples\features.lux:180
    if current == nil then
      current = 0
    end
    nextCount = current + 1
  end
  return nextCount
end
--#lux source: examples\features.lux:187
callbackExamples = function(panel)
--#lux source: examples\features.lux:188
  panel.Paint = function(self, w, h)
    local color
    if self.isActive then
      color = "green"
    else
      color = "gray"
    end
    print("paint", w, h, color)
  end
--#lux source: examples\features.lux:193
  panel.OnClick = function()
    local __lux_tmp_count_42 = panel.count
    if __lux_tmp_count_42 == nil then
      __lux_tmp_count_42 = 0
    end
    return helper(__lux_tmp_count_42)
  end
end
--#lux source: examples\features.lux:196
gmodGlobalsDemo = function(panel)
  hook.Add(
    "Think",
    "LuxFeatureTour",
    function()
      return nil
    end
  )
  timer.Simple(
    1,
    function()
      return print("later")
    end
  )
  local __lux_obj_panel_43 = panel
  local __lux_val_GetName_45 = nil
--#lux source: examples\features.lux:201
  if __lux_obj_panel_43 ~= nil then
    local __lux_method_GetName_44 = __lux_obj_panel_43.GetName
    if __lux_method_GetName_44 ~= nil then
      __lux_val_GetName_45 = __lux_method_GetName_44(__lux_obj_panel_43)
    end
  end
  local __lux_tmp_GetName_46 = __lux_val_GetName_45
  if __lux_tmp_GetName_46 == nil then
    __lux_tmp_GetName_46 = "unknown"
  end
  return { panelName = __lux_tmp_GetName_46, panelValid = panel ~= nil }
end
--#lux source: examples\features.lux:206
demo = function(player, panel, state, xs, keyFactory, ...)
  local summary = summarize(player, "Unknown")
  local kind = classify(player)
  local changed = mutateCounter(state)
  local suppressed = noImplicitReturn()
  local item = safeLookup(xs, keyFactory)
  local tier
  do
    local __lux_obj_player_47 = player
    local __lux_val_GetExp_49 = nil
--#lux source: examples\features.lux:212
    if __lux_obj_player_47 ~= nil then
      local __lux_method_GetExp_48 = __lux_obj_player_47.GetExp
      if __lux_method_GetExp_48 ~= nil then
        __lux_val_GetExp_49 = __lux_method_GetExp_48(__lux_obj_player_47)
      end
    end
    local __lux_tmp_GetExp_50 = __lux_val_GetExp_49
    if __lux_tmp_GetExp_50 == nil then
      __lux_tmp_GetExp_50 = 0
    end
    tier = tierForExp(__lux_tmp_GetExp_50)
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
  local piped = pipelineHelpers(xs)
  local debugged = debugValue(state.count)
  local tail = tailCallDemo(state.count)
  local destructured = defaultAndDestructure(player)
  local mergedProps = mergeTableProps({ align = "left" }, summary)
  local nextCount = doExpressionDemo(state)
  local gmodGlobals = gmodGlobalsDemo(panel)
  local first, second = multivalueDemo(...)
  return {
    version = featureVersion,
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
    piped = piped,
    debugged = debugged,
    tail = tail,
    destructured = destructured,
    mergedProps = mergedProps,
    nextCount = nextCount,
    gmodGlobals = gmodGlobals,
    first = first,
    second = second,
  }
end
--#lux source: examples\features.lux:260
multivalueDemo = function(...)
  local a, b = passthrough(...)
  local c, d = passthrough(...), passthrough(...)
  local packed = { "head", passthrough(...) }
  return passthrough(...), passthrough(...)
end
--#lux source: examples\features.lux:267
function PANEL:Paint(w, h)
  local name
  do
    local __lux_obj_self_51 = self
    local __lux_val_GetName_53 = nil
--#lux source: examples\features.lux:268
    if __lux_obj_self_51 ~= nil then
      local __lux_method_GetName_52 = __lux_obj_self_51.GetName
      if __lux_method_GetName_52 ~= nil then
        __lux_val_GetName_53 = __lux_method_GetName_52(__lux_obj_self_51)
      end
    end
    name = __lux_val_GetName_53
    if name == nil then
      name = "panel"
    end
  end
  print("paint", name, w, h)
end

return __lux_exports
