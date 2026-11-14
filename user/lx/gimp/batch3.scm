(let* ((image (car (gimp-file-load RUN-NONINTERACTIVE "in.png")))
       (drawable (vector-ref (car (gimp-image-get-selected-drawables image)) 0)))
  (gimp-drawable-merge-new-filter drawable "gegl:gaussian-blur" 0 LAYER-MODE-REPLACE 1.0
                                  "std-dev-x" 4.0 "std-dev-y" 4.0)
  (gimp-drawable-invert drawable FALSE)
  (file-png-export #:run-mode RUN-NONINTERACTIVE #:image image #:file "out.png"
                   #:interlaced FALSE #:compression 9 #:bkgd FALSE #:offs FALSE
                   #:phys FALSE #:time FALSE #:save-transparent FALSE
                   #:include-exif FALSE #:include-iptc FALSE #:include-xmp FALSE
                   #:include-color-profile FALSE #:include-thumbnail FALSE
                   #:include-comment FALSE)
  (gimp-image-delete image))
