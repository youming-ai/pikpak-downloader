package main

import "testing"

func TestParseSize(t *testing.T) {
	p := &PikPakClient{}

	cases := []struct {
		name    string
		input   string
		want    int64
		wantErr bool
	}{
		{"empty string returns zero", "", 0, false},
		{"plain integer bytes", "1234", 1234, false},
		{"KB suffix", "2KB", 2 * 1024, false},
		{"MB suffix", "3.5MB", int64(3.5 * 1024 * 1024), false},
		{"GB suffix", "1.25GB", int64(1.25 * 1024 * 1024 * 1024), false},
		{"scientific notation", "1e+9", 1_000_000_000, false},
		{"invalid bogus string", "notasize", 0, true},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			got, err := p.parseSize(tc.input)
			if tc.wantErr {
				if err == nil {
					t.Fatalf("parseSize(%q) expected error, got %d", tc.input, got)
				}
				return
			}
			if err != nil {
				t.Fatalf("parseSize(%q) unexpected error: %v", tc.input, err)
			}
			if got != tc.want {
				t.Errorf("parseSize(%q) = %d; want %d", tc.input, got, tc.want)
			}
		})
	}
}

func TestParseLongFormatLine(t *testing.T) {
	p := &PikPakClient{}

	cases := []struct {
		name     string
		line     string
		wantName string
		wantSize int64
		wantType FileType
	}{
		{
			name:     "file with MB size",
			line:     "drwxr-xr-x 1 12.5MB 2024-01-01 12:00 photo.jpg",
			wantName: "photo.jpg",
			wantSize: int64(12.5 * 1024 * 1024),
			wantType: TypeImage,
		},
		{
			name:     "file name contains spaces",
			line:     "drwxr-xr-x 1 500KB 2024-01-01 12:00 My Holiday Photos.zip",
			wantName: "My Holiday Photos.zip",
			wantSize: 500 * 1024,
			wantType: TypeArchive,
		},
		{
			name:     "plain integer size",
			line:     "drwxr-xr-x 1 4096 2024-01-01 12:00 folder_name",
			wantName: "folder_name",
			wantSize: 4096,
			wantType: TypeFolder,
		},
		{
			name:     "too few fields returns empty FileInfo",
			line:     "only three tokens",
			wantName: "",
			wantSize: 0,
			wantType: "",
		},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			got := p.parseLongFormatLine(tc.line)
			if got.Name != tc.wantName {
				t.Errorf("Name = %q; want %q", got.Name, tc.wantName)
			}
			if got.Size != tc.wantSize {
				t.Errorf("Size = %d; want %d", got.Size, tc.wantSize)
			}
			if got.Type != tc.wantType {
				t.Errorf("Type = %q; want %q", got.Type, tc.wantType)
			}
		})
	}
}

func TestParseQuotaOutput(t *testing.T) {
	p := &PikPakClient{}

	t.Run("typical header + data", func(t *testing.T) {
		out := `
   total   used
   10GB    2.5GB
`
		q, err := p.parseQuotaOutput(out)
		if err != nil {
			t.Fatalf("unexpected error: %v", err)
		}
		if q.Total != 10*1024*1024*1024 {
			t.Errorf("Total = %d; want %d", q.Total, int64(10)*1024*1024*1024)
		}
		wantUsed := int64(2.5 * 1024 * 1024 * 1024)
		if q.Used != wantUsed {
			t.Errorf("Used = %d; want %d", q.Used, wantUsed)
		}
	})

	t.Run("empty output yields zero quota without error", func(t *testing.T) {
		q, err := p.parseQuotaOutput("")
		if err != nil {
			t.Fatalf("unexpected error: %v", err)
		}
		if q.Total != 0 || q.Used != 0 {
			t.Errorf("expected zero quota, got total=%d used=%d", q.Total, q.Used)
		}
	})

	t.Run("missing data row after header", func(t *testing.T) {
		q, err := p.parseQuotaOutput("total used\n")
		if err != nil {
			t.Fatalf("unexpected error: %v", err)
		}
		if q.Total != 0 || q.Used != 0 {
			t.Errorf("expected zero quota, got total=%d used=%d", q.Total, q.Used)
		}
	})
}

func TestDetectFileType(t *testing.T) {
	p := &PikPakClient{}

	cases := []struct {
		filename string
		want     FileType
	}{
		{"photo.JPG", TypeImage},
		{"clip.mp4", TypeVideo},
		{"report.pdf", TypeDocument},
		{"archive.tar.gz", TypeArchive},
		{"README", TypeFolder}, // no extension => folder
		{"mystery.xyz", TypeOther},
	}

	for _, tc := range cases {
		t.Run(tc.filename, func(t *testing.T) {
			if got := p.detectFileType(tc.filename); got != tc.want {
				t.Errorf("detectFileType(%q) = %q; want %q", tc.filename, got, tc.want)
			}
		})
	}
}
