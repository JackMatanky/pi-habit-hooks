map(select(.lines > (.threshold // 500)))
| if length == 0 then []
  else
    [
      {
        smell: "oversized-file",
        details: {},
        issues: map({
          key: .file,
          details: {
            file: .file,
            line: .lines,
            message: ("File has " + (.lines | tostring) + " lines of code (comments and blank lines not counted), above the threshold of " + ((.threshold // 500) | tostring)),
            source: "line-count"
          }
        })
      }
    ]
  end
