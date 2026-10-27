;; Build:  frork compile examples/standalone/main.fnl -o demo
;; Run:    FRORK_DEMO_ROOT=/tmp/x ./demo [--apply]
(local {: ok : provide : utils} (require :frork))
(local {: path} (require :paths))
(import-macros {: ok-all} :macros)

;; A custom provider, using Lua's io library for the filesystem.
(provide :file
  {:status (fn [p contents]
             (case (io.open p)
               f (let [actual (f:read :a)]
                   (f:close)
                   (if (= actual contents)
                       :present
                       (values :drifted {:expected contents :actual actual})))
               _ :absent))
   :create (fn [p contents]
             (with-open [f (assert (io.open p :w))]
               (f:write contents)))
   :display (fn [p] (.. "file " p))})

(ok-all :directory (path "config") (path "config/app"))
(ok :file (path "config/app/settings.txt") (.. "built on " utils.platform "\n"))
(ok :symlink (path "settings.txt") (path "config/app/settings.txt"))
