return function(__lux_import)
  --#lux source: ..\examples\gmod_project\src\shared\hud.lux:1
  local __lux_exports = {}
  --#lux source: ..\examples\gmod_project\src\shared\hud.lux:9
  local formatCount
  --#lux source: ..\examples\gmod_project\src\shared\hud.lux:12
  local normalizeMode
  --#lux source: ..\examples\gmod_project\src\shared\hud.lux:18
  local modeTitle
  --#lux source: ..\examples\gmod_project\src\shared\hud.lux:24
  local formatPlayer
  --#lux source: ..\examples\gmod_project\src\shared\hud.lux:34
  local buildHud
  --#lux source: ..\examples\gmod_project\src\shared\hud.lux:1
  do
    local __lux_import_1 = __lux_import("lux/std#client")
    local arr = __lux_import_1.arr
  --#lux source: ..\examples\gmod_project\src\shared\hud.lux:2
    local __lux_import_2 = __lux_import("gmod_project/math#client")
    local add = __lux_import_2.add
    local values = __lux_import_2.values
  --#lux source: ..\examples\gmod_project\src\shared\hud.lux:9
    formatCount = function(count)
      return "Count: " .. tostring(count)
    end
  --#lux source: ..\examples\gmod_project\src\shared\hud.lux:12
    normalizeMode = function(mode)
      local __lux_match_3 = mode
      if __lux_match_3 == "detailed" then
        return "detailed"
      else
        return "compact"
      end
    end
  --#lux source: ..\examples\gmod_project\src\shared\hud.lux:18
    modeTitle = function(mode)
      local __lux_match_4 = mode
      if __lux_match_4 == "compact" then
        return "Compact HUD"
      elseif __lux_match_4 == "detailed" then
        return "Detailed HUD"
      end
    end
  --#lux source: ..\examples\gmod_project\src\shared\hud.lux:24
    formatPlayer = function(player, index, detailed)
      if player == nil then
        return "#" .. tostring(index) .. ": unknown"
      end
      local name
      do
        local __lux_obj_5 = player
        local __lux_val_7 = nil
        if __lux_obj_5 ~= nil then
          local __lux_method_6 = __lux_obj_5.Nick
          if __lux_method_6 ~= nil then
            __lux_val_7 = __lux_method_6(__lux_obj_5)
          end
        end
        local __lux_tmp_8 = __lux_val_7
        if __lux_tmp_8 == nil then
          __lux_tmp_8 = "unknown"
        end
        name = __lux_tmp_8
      end
      local __lux_match_9 = detailed
      if __lux_match_9 == true then
        local __lux_obj_10 = player
        local __lux_val_12 = nil
        if __lux_obj_10 ~= nil then
          local __lux_method_11 = __lux_obj_10.Health
          if __lux_method_11 ~= nil then
            __lux_val_12 = __lux_method_11(__lux_obj_10)
          end
        end
        local __lux_tmp_13 = __lux_val_12
        if __lux_tmp_13 == nil then
          __lux_tmp_13 = 0
        end
        return "#" ..
          tostring(index) ..
          ": " ..
          tostring(name) ..
          " (" ..
          tostring(__lux_tmp_13) ..
          " hp)"
      elseif __lux_match_9 == false then
        return "#" .. tostring(index) .. ": " .. tostring(name)
      end
    end
  --#lux source: ..\examples\gmod_project\src\shared\hud.lux:34
    buildHud = function(count, players, mode, ...)
      local normalizedMode = normalizeMode(mode)
      local detailed = normalizedMode == "detailed"
      local total = add(count, #players)
      local labels = arr.map(
        players,
        function(player, index)
          return formatPlayer(player, index, detailed)
        end
      )
      local first, second = values(...)
      return {
        total = total,
        mode = normalizedMode,
        title = modeTitle(normalizedMode),
        text = formatCount(total),
        labels = labels,
        first = first,
        second = second,
      }
    end
  --#lux source: ..\examples\gmod_project\src\shared\hud.lux:1
  end
  
  --#lux source: ..\examples\gmod_project\src\shared\hud.lux:34
  __lux_exports.buildHud = buildHud
  
  --#lux source: ..\examples\gmod_project\src\shared\hud.lux:1
  return __lux_exports
end
