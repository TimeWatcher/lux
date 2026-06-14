return function(__lux_import)
  local __lux_exports = {}
  local arr
  local dict
  local set
  local str
  local num
  local func
  local pool
  do
    arr = {}
    dict = {}
    set = {}
    str = {}
    num = {}
    func = {}
    pool = {}
    function arr.clear(input)
      for index = #input, 1, -1 do
        input[index] = nil
      end
      return input
    end
    function arr.copyInto(input, out)
      arr.clear(out)
      for index = 1, #input do
        out[index] = input[index]
      end
      return out
    end
    function arr.copy(input)
      local out = {}
      return arr.copyInto(input, out)
    end
    function arr.mapInto(input, callback, out)
      arr.clear(out)
      for index = 1, #input do
        out[index] = callback(input[index], index)
      end
      return out
    end
    function arr.map(input, callback)
      local out = {}
      return arr.mapInto(input, callback, out)
    end
    function arr.filterInto(input, callback, out)
      arr.clear(out)
      local outIndex = 1
      for index = 1, #input do
        local value = input[index]
        if callback(value, index) then
          out[outIndex] = value
          outIndex = outIndex + 1
        end
      end
      return out
    end
    function arr.filter(input, callback)
      local out = {}
      return arr.filterInto(input, callback, out)
    end
    function arr.reduce(input, initial, callback)
      local acc = initial
      local start = 1
      if acc == nil then
        acc = input[1]
        start = 2
      end
      for index = start, #input do
        acc = callback(acc, input[index], index)
      end
      return acc
    end
    function arr.some(input, callback)
      for index = 1, #input do
        if callback(input[index], index) then
          return true
        end
      end
      return false
    end
    function arr.every(input, callback)
      for index = 1, #input do
        if not callback(input[index], index) then
          return false
        end
      end
      return true
    end
    function arr.find(input, callback)
      for index = 1, #input do
        local value = input[index]
        if callback(value, index) then
          return value, index
        end
      end
      return nil, nil
    end
    function arr.indexOf(input, needle)
      for index = 1, #input do
        if input[index] == needle then
          return index
        end
      end
      return nil
    end
    function arr.contains(input, needle)
      return arr.indexOf(input, needle) ~= nil
    end
    function arr.removeValue(input, needle)
      local index = arr.indexOf(input, needle)
      if index == nil then
        return false
      end
      table.remove(input, index)
      return true
    end
    function arr.removeWhereInPlace(input, callback)
      local removed = 0
      for index = #input, 1, -1 do
        if callback(input[index], index) then
          table.remove(input, index)
          removed = removed + 1
        end
      end
      return removed
    end
    function arr.compactInPlace(input)
      local outIndex = 1
      for index = 1, #input do
        local value = input[index]
        if value ~= nil then
          input[outIndex] = value
          outIndex = outIndex + 1
        end
      end
      for index = #input, outIndex, -1 do
        input[index] = nil
      end
      return input
    end
    function arr.pushAllInto(out, input)
      local offset = #out
      for index = 1, #input do
        out[offset + index] = input[index]
      end
      return out
    end
    function dict.clear(input)
      for key in pairs(input) do
        input[key] = nil
      end
      return input
    end
    function dict.count(input)
      local total = 0
      for _key in pairs(input) do
        total = total + 1
      end
      return total
    end
    function dict.isEmpty(input)
      for _key in pairs(input) do
        return false
      end
      return true
    end
    function dict.keysInto(input, out)
      arr.clear(out)
      local index = 1
      for key in pairs(input) do
        out[index] = key
        index = index + 1
      end
      return out
    end
    function dict.keys(input)
      local out = {}
      return dict.keysInto(input, out)
    end
    function dict.valuesInto(input, out)
      arr.clear(out)
      local index = 1
      for _key, value in pairs(input) do
        out[index] = value
        index = index + 1
      end
      return out
    end
    function dict.values(input)
      local out = {}
      return dict.valuesInto(input, out)
    end
    function dict.copyInto(input, out)
      dict.clear(out)
      for key, value in pairs(input) do
        out[key] = value
      end
      return out
    end
    function dict.copy(input)
      local out = {}
      return dict.copyInto(input, out)
    end
    function dict.mergeInto(out, ...)
      for index = 1, select("#", ...) do
        local source = select(index, ...)
        if source ~= nil then
          for key, value in pairs(source) do
            out[key] = value
          end
        end
      end
      return out
    end
    function dict.merge(...)
      local out = {}
      return dict.mergeInto(out, ...)
    end
    function dict.defaultsInto(out, defaults)
      if defaults ~= nil then
        for key, value in pairs(defaults) do
          if out[key] == nil then
            out[key] = value
          end
        end
      end
      return out
    end
    function dict.pickInto(input, keys, out)
      dict.clear(out)
      for index = 1, #keys do
        local key = keys[index]
        out[key] = input[key]
      end
      return out
    end
    function dict.pick(input, keys)
      local out = {}
      return dict.pickInto(input, keys, out)
    end
    function dict.omitInto(input, omitted, out)
      dict.clear(out)
      for key, value in pairs(input) do
        if omitted[key] ~= true then
          out[key] = value
        end
      end
      return out
    end
    function dict.omit(input, omitted)
      local out = {}
      return dict.omitInto(input, omitted, out)
    end
    function dict.shallowEqual(left, right)
      if left == right then
        return true
      end
      local __lux_tmp_1 = left == nil
      if not __lux_tmp_1 then
        __lux_tmp_1 = right == nil
      end
      if __lux_tmp_1 then
        return false
      end
      for key, value in pairs(left) do
        if right[key] ~= value then
          return false
        end
      end
      for key, value in pairs(right) do
        if left[key] ~= value then
          return false
        end
      end
      return true
    end
    function set.fromArray(input)
      local out = {}
      for index = 1, #input do
        out[input[index]] = true
      end
      return out
    end
    function set.add(input, value)
      input[value] = true
      return input
    end
    function set.remove(input, value)
      input[value] = nil
      return input
    end
    function set.has(input, value)
      return input[value] == true
    end
    function set.unionInto(out, left, right)
      dict.clear(out)
      for value in pairs(left) do
        out[value] = true
      end
      for value in pairs(right) do
        out[value] = true
      end
      return out
    end
    function set.intersectInto(out, left, right)
      dict.clear(out)
      for value in pairs(left) do
        if right[value] == true then
          out[value] = true
        end
      end
      return out
    end
    function set.differenceInto(out, left, right)
      dict.clear(out)
      for value in pairs(left) do
        if right[value] ~= true then
          out[value] = true
        end
      end
      return out
    end
    function str.startsWith(value, prefix)
      return string.sub(value, 1, #prefix) == prefix
    end
    function str.endsWith(value, suffix)
      if suffix == "" then
        return true
      end
      return string.sub(value, -#suffix) == suffix
    end
    function str.contains(value, needle)
      return string.find(value, needle, 1, true) ~= nil
    end
    function str.trim(value)
      return string.match(value, "^%s*(.-)%s*$")
    end
    function str.escapePattern(value)
      return string.gsub(value, "([^%w])", "%%%1")
    end
    function str.splitInto(value, separator, out)
      arr.clear(out)
      if separator == "" then
        out[1] = value
        return out
      end
      local start = 1
      local outIndex = 1
      while true do
        local first, last = string.find(value, separator, start, true)
        if first == nil then
          out[outIndex] = string.sub(value, start)
          return out
        end
        out[outIndex] = string.sub(value, start, first - 1)
        outIndex = outIndex + 1
        start = last + 1
      end
    end
    function str.split(value, separator)
      local out = {}
      return str.splitInto(value, separator, out)
    end
    function str.join(input, separator)
      return table.concat(input, separator)
    end
    function str.truncate(value, maxLength, suffix)
      if suffix == nil then
        suffix = "..."
      end
      if #value <= maxLength then
        return value
      end
      if maxLength <= #suffix then
        return string.sub(value, 1, maxLength)
      end
      return string.sub(value, 1, maxLength - #suffix) .. suffix
    end
    function str.formatBytes(bytes)
      local units = { "B", "KB", "MB", "GB" }
      local value = bytes
      local unit = 1
      while true do
        do
          local __lux_tmp_2 = value >= 1024
          if __lux_tmp_2 then
            __lux_tmp_2 = unit < #units
          end
          if not (__lux_tmp_2) then
            break
          end
        end
        value = value / 1024
        unit = unit + 1
      end
      if unit == 1 then
        return tostring(value) .. " " .. units[unit]
      end
      return string.format("%.1f %s", value, units[unit])
    end
    function num.clamp(value, minValue, maxValue)
      if value < minValue then
        return minValue
      end
      if value > maxValue then
        return maxValue
      end
      return value
    end
    function num.lerp(t, from, to)
      return from + (to - from) * t
    end
    function num.round(value, step)
      if step == nil then
        step = 1
      end
      return math.floor(value / step + 0.5) * step
    end
    function num.sign(value)
      if value > 0 then
        return 1
      end
      if value < 0 then
        return -1
      end
      return 0
    end
    function num.approach(current, target, delta)
      if current < target then
        return math.min(current + delta, target)
      end
      if current > target then
        return math.max(current - delta, target)
      end
      return target
    end
    function num.remap(value, inMin, inMax, outMin, outMax)
      if inMax == inMin then
        return outMin
      end
      return outMin + (value - inMin) * (outMax - outMin) / (inMax - inMin)
    end
    function num.nearlyEqual(left, right, epsilon)
      if epsilon == nil then
        epsilon = 0.000001
      end
      local __lux_cmp_3 = false
      if math.abs(left - right) ~= nil and epsilon ~= nil then
        __lux_cmp_3 = math.abs(left - right) <= epsilon
      end
      return __lux_cmp_3
    end
    function func.noop()
      return nil
    end
    function func.identity(value)
      return value
    end
    function func.once(callback)
      local called = false
      local result = nil
      return function(...)
        if not called then
          called = true
          result = callback(...)
        end
        return result
      end
    end
    function func.try(callback, fallback)
      local ok, result = pcall(callback)
      if ok then
        return result
      end
      return fallback
    end
    function pool.clearTable(input)
      return dict.clear(input)
    end
    function pool.new()
      return { items = {}, count = 0 }
    end
    function pool.acquire(bucket)
      if bucket.count > 0 then
        local item = bucket.items[bucket.count]
        bucket.items[bucket.count] = nil
        bucket.count = bucket.count - 1
        return item
      end
      return {}
    end
    function pool.release(bucket, item, clear)
      if clear == nil then
        clear = true
      end
      if item == nil then
        return bucket
      end
      if clear then
        dict.clear(item)
      end
      bucket.count = bucket.count + 1
      bucket.items[bucket.count] = item
      return bucket
    end
  end
  
  __lux_exports.arr = arr
  __lux_exports.dict = dict
  __lux_exports.set = set
  __lux_exports.str = str
  __lux_exports.num = num
  __lux_exports.func = func
  __lux_exports.pool = pool
  
  return __lux_exports
end
