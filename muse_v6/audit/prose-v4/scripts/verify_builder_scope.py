#!/usr/bin/env python3
import ast
import re
import sys
from pathlib import Path

PATH=Path(__file__).resolve().with_name('build_strict_audit.py')
source=PATH.read_text()
marks=list(re.finditer(r'^# (\d{2})\s*$',source,re.M))
# Names deliberately provided by the shared audit DSL.
provided={
    'rows','spec','T','O','ST','ATT','MOD','CAP','IQ','PHASE','CAUSE','TEMP','NEG',
    'AND','OR','EQ','QUANT','SA','OPGRAM','QUOTE','IMP','sp','sp_in','sp_near',
    'spans_union','term','add','True','False','None','range','len','str','dict',
    'list','tuple','set','min','max',
}
errors=[]

class BlockVisitor(ast.NodeVisitor):
    def __init__(self):
        self.assigned={'T'}
        self.locals=[]
    def load(self,name,line):
        if name not in self.assigned and name not in provided and not any(name in scope for scope in self.locals):
            errors.append((sample,name,line))
    def visit_Name(self,node):
        if isinstance(node.ctx,ast.Load): self.load(node.id,node.lineno)
    def assign_target(self,target):
        if isinstance(target,ast.Name):
            self.assigned.add(target.id)
        elif isinstance(target,(ast.Tuple,ast.List)):
            for elt in target.elts:self.assign_target(elt)
        else:
            self.visit(target)
    def visit_Assign(self,node):
        self.visit(node.value)
        for target in node.targets:self.assign_target(target)
    def visit_FunctionDef(self,node):
        self.assigned.add(node.name)
        # Function bodies have their own locals and do not participate in sample top-level dataflow.
    def visit_ListComp(self,node):
        names=set()
        for gen in node.generators:
            if isinstance(gen.target,ast.Name): names.add(gen.target.id)
        self.locals.append(names)
        self.visit(node.elt)
        for gen in node.generators:
            self.visit(gen.iter)
            for cond in gen.ifs:self.visit(cond)
        self.locals.pop()

for index,mark in enumerate(marks):
    sample=int(mark.group(1))
    start=mark.end()
    end=marks[index+1].start() if index+1<len(marks) else source.find('# Remaining samples',start)
    if end<0:end=len(source)
    block=source[start:end]
    tree=ast.parse(block)
    BlockVisitor().visit(tree)

if errors:
    for sample,name,line in errors:
        print(f'S{sample}: read of {name!r} before assignment in this sample block (block line {line})')
    sys.exit(1)
print(f'audit builder scope verification passed: {len(marks)} isolated sample blocks')
