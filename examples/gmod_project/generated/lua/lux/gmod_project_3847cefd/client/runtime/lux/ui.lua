return function(__lux_import)
  local __lux_exports = {}
  local valueOf
  local appendChild
  local normalizeChildren
  local node
  local text
  local Text
  local fragment
  local primitive
  local component
  local bind
  local mount
  local Show
  local showNode
  local For
  local forNode
  local Column
  local Row
  local Label
  local Button
  local Spacer
  local Panel
  do
    local __lux_import_1 = __lux_import("lux/reactive#client")
    local effect = __lux_import_1.effect
    valueOf = function(value)
      if type(value) == "function" then
        return value()
      end
      return value
    end
    appendChild = function(out, child)
      local __lux_tmp_2 = child == nil
      if not __lux_tmp_2 then
        __lux_tmp_2 = child == false
      end
      if __lux_tmp_2 then
        return out
      end
      if type(child) == "table" then
        if child.__lux_ui_fragment == true then
          do
            local __lux_tmp_3 = child.children
            if __lux_tmp_3 == nil then
              __lux_tmp_3 = {}
            end
            for _, nested in ipairs(__lux_tmp_3) do
              appendChild(out, nested)
            end
          end
          return out
        end
        out[#out + 1] = child
        return out
      end
      out[#out + 1] = text(child)
      return out
    end
    normalizeChildren = function(children)
      local out = {}
      local __lux_tmp_4 = children == nil
      if not __lux_tmp_4 then
        __lux_tmp_4 = children == false
      end
      if __lux_tmp_4 then
        return out
      end
      if type(children) ~= "table" then
        appendChild(out, children)
        return out
      end
      if children.__lux_ui_fragment == true then
        return normalizeChildren(children.children)
      end
      for _, child in ipairs(children) do
        appendChild(out, child)
      end
      return out
    end
    node = function(kind, props, children)
      local __lux_tmp_5 = props
      if __lux_tmp_5 == nil then
        __lux_tmp_5 = {}
      end
      return {
        __lux_ui_node = true,
        kind = kind,
        props = __lux_tmp_5,
        children = normalizeChildren(children),
      }
    end
    text = function(value)
      return node("Text", { value = tostring(value) }, {})
    end
    Text = function(props)
      return primitive("Text", props)
    end
    fragment = function(children)
      return { __lux_ui_fragment = true, children = normalizeChildren(children) }
    end
    primitive = function(kind, props)
      local initialProps
      do
        local __lux_tmp_6 = props
        if __lux_tmp_6 == nil then
          __lux_tmp_6 = {}
        end
        initialProps = __lux_tmp_6
      end
      local item = node(kind, initialProps, {})
      return setmetatable(
        item,
        {
          __call = function(self, children)
            return node(kind, initialProps, children)
          end,
        }
      )
    end
    component = function(render, props, children)
      local __lux_tmp_7 = props
      if __lux_tmp_7 == nil then
        __lux_tmp_7 = {}
      end
      return render(__lux_tmp_7, normalizeChildren(children))
    end
    bind = function(read)
      return { __lux_ui_binding = true, read = read }
    end
    mount = function(producer, apply)
      return effect(
        function()
          return apply(producer())
        end
      )
    end
    Show = function(props)
      local initialProps
      do
        local __lux_tmp_8 = props
        if __lux_tmp_8 == nil then
          __lux_tmp_8 = {}
        end
        initialProps = __lux_tmp_8
      end
      local item = showNode(initialProps, {})
      return setmetatable(
        item,
        {
          __call = function(self, children)
            return showNode(initialProps, children)
          end,
        }
      )
    end
    showNode = function(props, children)
      if valueOf(props.when) then
        return fragment(children)
      end
      local __lux_tmp_9 = props.fallback
      if __lux_tmp_9 == nil then
        __lux_tmp_9 = fragment({})
      end
      return __lux_tmp_9
    end
    For = function(props)
      local initialProps
      do
        local __lux_tmp_10 = props
        if __lux_tmp_10 == nil then
          __lux_tmp_10 = {}
        end
        initialProps = __lux_tmp_10
      end
      local item = forNode(initialProps, {})
      return setmetatable(
        item,
        {
          __call = function(self, children)
            return forNode(initialProps, children)
          end,
        }
      )
    end
    forNode = function(props, children)
      local each
      do
        local __lux_tmp_11 = valueOf(props.each)
        if __lux_tmp_11 == nil then
          __lux_tmp_11 = {}
        end
        each = __lux_tmp_11
      end
      local render
      do
        local __lux_tmp_12 = props.children
        if __lux_tmp_12 == nil then
          __lux_tmp_12 = props.render
        end
        render = __lux_tmp_12
      end
      local out = {}
      if render == nil then
        return fragment(children)
      end
      for index, item in ipairs(each) do
        appendChild(out, render(item, index))
      end
      return fragment(out)
    end
    Column = function(props)
      return primitive("Column", props)
    end
    Row = function(props)
      return primitive("Row", props)
    end
    Label = function(props)
      return primitive("Label", props)
    end
    Button = function(props)
      return primitive("Button", props)
    end
    Spacer = function(props)
      return primitive("Spacer", props)
    end
    Panel = function(props)
      return primitive("Panel", props)
    end
  end
  
  __lux_exports.node = node
  __lux_exports.Text = Text
  __lux_exports.fragment = fragment
  __lux_exports.component = component
  __lux_exports.bind = bind
  __lux_exports.mount = mount
  __lux_exports.Show = Show
  __lux_exports.For = For
  __lux_exports.Column = Column
  __lux_exports.Row = Row
  __lux_exports.Label = Label
  __lux_exports.Button = Button
  __lux_exports.Spacer = Spacer
  __lux_exports.Panel = Panel
  
  return __lux_exports
end
