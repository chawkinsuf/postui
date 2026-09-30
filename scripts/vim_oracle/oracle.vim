" Runs every case in g:oracle_in through Vim and writes one JSON line per
" case to g:oracle_out; the first line is a header. Started by
" generate.py; spec §6.3 says why each step is here.
let s:cases = json_decode(join(readfile(g:oracle_in), "\n"))
let s:out = [json_encode({'header': {
      \ 'versionlong': v:versionlong,
      \ 'winheight': winheight(0),
      \ }})]

" Runs inside the final mode (trap 3): feedkeys' 'x' flag ends Insert and
" Visual afterwards, so nothing may be read after it returns.
function! Capture() abort
  let l:mode = mode(1)
  let g:oracle_cap = {
        \ 'mode': l:mode,
        \ 'lines': getline(1, '$'),
        \ 'cursor': [line('.'), charcol('.')],
        \ 'visual': l:mode =~# '^[vV]' ? getcharpos('v')[1:2] : v:null,
        \ 'reg': getreg('"'),
        \ 'regtype': getregtype('"'),
        \ 'top': line('w0'),
        \ }
endfunction

for s:c in s:cases
  enew!
  setlocal buftype=nofile noswapfile
  " `.` survives buffers and cannot be cleared (trap 8), so a case whose
  " change fails would replay the previous case's change. Prime it with a
  " change no corpus text can repeat: `df☃` finds no ☃ (proven 2026-09-29).
  call setline(1, "a☃")
  execute "normal! 0df☃"
  " Set the text with undo off, which also clears the history (:help
  " clear-undo), so `u` cannot undo the setup. An undo break alone
  " (`let &undolevels = &undolevels`) does not stop that (proven 2026-09-29).
  let s:ul = &undolevels
  set undolevels=-1
  call setline(1, s:c.lines)
  let &undolevels = s:ul
  " State that outlives a buffer (trap 8).
  call setreg('"', '')
  call setreg('0', '')
  call setcharsearch({'char': ''})
  if type(s:c.reg) == v:t_dict
    call setreg('"', s:c.reg.text, s:c.reg.type)
  endif
  call cursor(s:c.cursor[0], 1)
  call setcursorcharpos(s:c.cursor[0], s:c.cursor[1])
  let v:errmsg = ''
  let g:oracle_cap = v:null
  " Each <Name> token becomes its key inside Vim, so a literal `"` or `\`
  " in the keys needs no escaping.
  let s:keys = join(map(copy(s:c.tokens),
        \ {_, t -> t =~# '^<.\+>$' ? eval('"\' . t . '"') : t}), '')
  try
    call feedkeys(s:keys . "\<Cmd>call Capture()\<CR>", 'ntx')
  catch
  endtry
  if mode(1) !=# 'n'
    execute "normal! \<Esc>"
  endif
  call add(s:out, json_encode({'id': s:c.id, 'capture': g:oracle_cap, 'errmsg': v:errmsg}))
  bwipeout!
endfor
call writefile(s:out, g:oracle_out)
qa!
