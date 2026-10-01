" Writes every code point whose Vim toupper() or tolower() is another code
" point to g:out, one "code upper lower" line each (decimal). Run by
" `generate.py --cases`, which passes SETTINGS_LINE (and so 'casemap') with
" -c. Surrogates are skipped. Plan 3b.
let s:out = []
let s:c = 1
while s:c <= 0x10FFFF
  if s:c < 0xD800 || s:c > 0xDFFF
    let s:ch = nr2char(s:c)
    let s:u = char2nr(toupper(s:ch))
    let s:l = char2nr(tolower(s:ch))
    if s:u != s:c || s:l != s:c
      call add(s:out, printf('%d %d %d', s:c, s:u, s:l))
    endif
  endif
  let s:c += 1
endwhile
call writefile(s:out, g:out)
qa!
