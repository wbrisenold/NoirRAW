#!/usr/bin/env python3
"""Portable contract smoke: Python reference of the Rust/WGSL signed-log mapping."""
from math import log2,exp2

def encode(x):
    if x < 0: return .2*(1-log2(1+min(-x,2)*8)/log2(17))
    return .2+.8*log2(1+min(x,64)*8)/log2(513)
def decode(u):
    if u < .2:return -(exp2((.2-u)/.2*log2(17))-1)/8
    return (exp2((u-.2)/.8*log2(513))-1)/8

if __name__=='__main__':
    for x in [-2.,-.25,0.,.001,.18,1.,4.,16.,64.]:
        y=decode(encode(x))
        assert abs(x-y)<1e-4,(x,y)
    print('SHAPER_SMOKE_PASS: nine signed-log values roundtrip')
