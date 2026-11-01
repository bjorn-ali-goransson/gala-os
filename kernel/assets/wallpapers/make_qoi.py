# QOI encoder written from the QOI specification (qoiformat.org), RGB.
import struct, sys
from PIL import Image
def encode(img):
    w,h=img.size; px=img.convert('RGB').tobytes()
    out=bytearray(b'qoif'+struct.pack('>II',w,h)+bytes([3,0]))
    index=[(0,0,0,0)]*64; prev=(0,0,0,255); run=0
    n=w*h
    for i in range(n):
        p=(px[3*i],px[3*i+1],px[3*i+2],255)
        if p==prev:
            run+=1
            if run==62 or i==n-1: out.append(0xc0|(run-1)); run=0
            continue
        if run: out.append(0xc0|(run-1)); run=0
        h_=(p[0]*3+p[1]*5+p[2]*7+p[3]*11)%64
        if index[h_]==p: out.append(h_)
        else:
            index[h_]=p
            dr=(p[0]-prev[0]+128)%256-128; dg=(p[1]-prev[1]+128)%256-128; db=(p[2]-prev[2]+128)%256-128
            if -2<=dr<=1 and -2<=dg<=1 and -2<=db<=1:
                out.append(0x40|((dr+2)<<4)|((dg+2)<<2)|(db+2))
            else:
                dr_dg=dr-dg; db_dg=db-dg
                if -32<=dg<=31 and -8<=dr_dg<=7 and -8<=db_dg<=7:
                    out.append(0x80|(dg+32)); out.append(((dr_dg+8)<<4)|(db_dg+8))
                else: out+=bytes([0xfe,p[0],p[1],p[2]])
        prev=p
    if run: out.append(0xc0|(run-1))
    out+=bytes([0]*7+[1]); return bytes(out)
src,dst,W,H=sys.argv[1],sys.argv[2],int(sys.argv[3]),int(sys.argv[4])
im=Image.open(src).convert('RGB')
# Crop to the target's aspect, centred, then scale.
sw,sh=im.size; tw=sw; th=sw*H//W
if th>sh: th=sh; tw=sh*W//H
l=(sw-tw)//2; t=(sh-th)//2
im=im.crop((l,t,l+tw,t+th)).resize((W,H),Image.LANCZOS)
open(dst,'wb').write(encode(im))
