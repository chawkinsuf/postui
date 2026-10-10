" Runs every response case in g:oracle_in through Vim and writes one JSON
" line per case to g:oracle_out; the first line is a header. Started by
" response.py. The traps are oracle.vim's (spec §6.3 of the text engine
" design): state that outlives a buffer is reset per case.
"
" Unlike oracle.vim, the keys are not run by `feedkeys(…, 'x')`, which runs
" them in `exec_normal()` and skips what Vim's main loop does between
" commands when they are typed: including a closed fold in the Visual area
" (`foldAdjustVisual()`), among others. Each case's keys go into the
" typeahead and Vim's main loop runs them as typed; the case ends with a
" <Cmd> that captures, then sets up and queues the next case. The typeahead
" never runs dry, so Vim never reads its (empty) input.
let s:cases = json_decode(join(readfile(g:oracle_in), "\n"))
let s:out = [json_encode({'header': {
      \ 'versionlong': v:versionlong,
      \ 'winheight': winheight(0),
      \ }})]
let s:i = -1

" The closed folds as the screen shows them: the outermost only.
function! ClosedFolds() abort
  let l:res = []
  let l:l = 1
  while l:l <= line('$')
    let l:c = foldclosed(l:l)
    if l:c != -1
      call add(l:res, [l:c, foldclosedend(l:l)])
      let l:l = foldclosedend(l:l) + 1
    else
      let l:l += 1
    endif
  endwhile
  return l:res
endfunction

function! Capture() abort
  let l:mode = mode(1)
  let g:oracle_cap = {
        \ 'mode': l:mode,
        \ 'cursor': [line('.'), charcol('.')],
        \ 'top': line('w0'),
        \ 'visual': l:mode =~# '^[vV]' ? getcharpos('v')[1:2] : v:null,
        \ 'reg': getreg('"'),
        \ 'regtype': getregtype('"'),
        \ 'search': @/,
        \ 'fwd': v:searchforward,
        \ 'closed': ClosedFolds(),
        \ 'msgs': split(execute('messages'), "\n"),
        \ }
endfunction

function! Next() abort
  if s:i >= 0
    call add(s:out, json_encode({'id': s:cases[s:i].id, 'capture': g:oracle_cap}))
    call setline(1, 'wiped')
    bwipeout!
  endif
  let s:i += 1
  if s:i >= len(s:cases)
    call writefile(s:out, g:oracle_out)
    qa!
  endif
  let l:c = s:cases[s:i]
  enew!
  setlocal buftype=nofile noswapfile foldmethod=manual foldlevel=0
  let l:ul = &undolevels
  set undolevels=-1
  call setline(1, l:c.lines)
  let &undolevels = l:ul
  " One manual fold per container, inner ones first, then all open: the
  " tree's folds, as it first shows them.
  for l:f in l:c.folds
    execute l:f[0] . ',' . l:f[1] . 'fold'
  endfor
  normal! zR
  call setreg('"', '')
  call setreg('0', '')
  setlocal scroll=0
  let @/ = ''
  let v:searchforward = 1
  call histdel('/')
  call cursor(1, 1)
  normal! zt
  " The message history, in order: its last line is what the message line
  " shows when the keys are done.
  messages clear
  let g:oracle_cap = v:null
  let l:keys = join(map(copy(l:c.tokens),
        \ {_, t -> t =~# '^<.\+>$' ? eval('"\' . t . '"') : t}), '')
  call feedkeys(l:keys . "\<Cmd>call Capture()\<CR>\<C-\>\<C-n>\<Cmd>call Next()\<CR>", 'nt')
endfunction

call Next()
