" Writes Vim's charclass() for every code point to g:out as compressed
" ranges, one "first last class" line each (decimal). Surrogates get -2
" and generate.py drops them. Run by `generate.py --classes`, which passes
" SETTINGS_LINE (and so 'iskeyword') with -c.
enew
let s:out = []
let s:start = 0
let s:cls = -1
let s:c = 0
while s:c <= 0x10FFFF
  if s:c >= 0xD800 && s:c <= 0xDFFF
    let s:k = -2
  elseif s:c == 0
    let s:k = 0
  else
    call setline(1, nr2char(s:c))
    let s:k = charclass(getline(1))
  endif
  if s:k != s:cls
    if s:cls != -1
      call add(s:out, printf('%d %d %d', s:start, s:c - 1, s:cls))
    endif
    let s:start = s:c
    let s:cls = s:k
  endif
  let s:c += 1
endwhile
call add(s:out, printf('%d %d %d', s:start, 0x10FFFF, s:cls))
call writefile(s:out, g:out)
qa!
