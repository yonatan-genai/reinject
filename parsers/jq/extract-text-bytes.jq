# Extract text bytes from a JSONL transcript line.
# Input: raw line (-R flag), one per JSONL entry.
# Output: "<non_thinking_bytes> <thinking_bytes>" per valid line.
# Invalid JSON lines are silently skipped (concurrent write resilience).

def byte_len: if . == null then 0 elif type == "string" then length else 0 end;

def count_content:
  .message.content // "" |
  if type == "string" then [length, 0]
  elif type == "array" then
    reduce .[] as $item ([0, 0];
      if ($item.type // "") == "thinking" then
        .[1] += (($item.thinking // "") | byte_len)
      elif ($item.type // "") == "text" or (($item.type // "") == "" and ($item | has("text"))) then
        .[0] += (($item.text // "") | byte_len)
      elif ($item.type // "") == "tool_use" then
        .[0] += (($item.input // {} | tojson) | byte_len)
      elif ($item.type // "") == "tool_result" then
        if ($item.content | type) == "string" then
          .[0] += ($item.content | byte_len)
        elif ($item.content | type) == "array" then
          .[0] += (reduce ($item.content[]?) as $sub (0;
            . + (($sub.text // "") | byte_len)
          ))
        else .
        end
      else .
      end
    )
  else [0, 0]
  end;

try (fromjson | count_content | "\(.[0]) \(.[1])") // empty
