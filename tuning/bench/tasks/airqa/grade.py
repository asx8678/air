import os, sys
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "lib"))
import grade
grade.airqa(sys.argv[1], sys.argv[2])
