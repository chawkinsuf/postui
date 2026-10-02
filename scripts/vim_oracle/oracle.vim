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
  " The window's 'scroll' (a counted ctrl+d/ctrl+u sets it), the last search
  " pattern and the search direction outlive a buffer too (trap 12).
  setlocal scroll=0
  let @/ = ''
  let v:searchforward = 1
  " Trap 14: the search history outlives a case, so `/<Up>` would recall the
  " last case's pattern (proven 2026-10-01: `/xyz<CR>` then `/<Up>foo<CR>`
  " searched `xyzfoo`).
  call histdel('/')
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
  " CTRL-\ CTRL-N after the capture, inside the same feedkeys: it ends any
  " mode and clears a pending Insert restart (a case ending inside Insert
  " ctrl+o, or in Visual entered from it), which `:normal! <Esc>` cannot,
  " since `:normal` saves and restores it (trap 13).
  " Trap 15: no `try` (and no `silent!`) around it. Inside a `try` an error
  " (E486) becomes an exception, and with `silent!` it returns early: either
  " way `emsg()` skips `flush_buffers()`, so a `.` whose search fails runs the
  " rest of its redo as commands, which real Vim never does (proven
  " 2026-10-01: `c/o<CR>Y<Esc>j.` yanked the line under both, not plain).
  call feedkeys(s:keys . "\<Cmd>call Capture()\<CR>\<C-\>\<C-n>", 'ntx')
  call add(s:out, json_encode({'id': s:c.id, 'capture': g:oracle_cap, 'errmsg': v:errmsg}))
  " Wiping the only buffer opens an empty one in its place, and Vim reuses
  " the current buffer for that when it is empty: a case that ends with the
  " text empty would hand the next case its Visual area and marks (trap 11).
  call setline(1, 'wiped')
  bwipeout!
endfor
call writefile(s:out, g:oracle_out)
qa!
