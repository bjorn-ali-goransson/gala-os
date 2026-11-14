; GIMP's batch run of scenario lx_gimp_batch (user/lx/gimp/batch.sh):
; open in.png (file-png, a plug-in process), blur it (GEGL's
; gaussian-blur), invert it and save it as out.png without the metadata
; that would change from run to run (gimprc: no Exif, XMP or profile).
(let* ((image (car (gimp-file-load RUN-NONINTERACTIVE "in.png" "in.png")))
       (drawable (car (gimp-image-get-active-drawable image))))
  (plug-in-gauss RUN-NONINTERACTIVE image drawable 4 4 0)
  (gimp-drawable-invert drawable FALSE)
  (file-png-save2 RUN-NONINTERACTIVE image drawable "out.png" "out.png"
                  0 9 0 0 0 0 0 0 0)
  (gimp-image-delete image))
